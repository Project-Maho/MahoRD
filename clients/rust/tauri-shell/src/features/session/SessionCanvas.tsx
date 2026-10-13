import { useEffect, useRef } from "react";
import { canvasDisplayRect, displayRect, type PresentationMode, type CursorMode } from "./presentation";
import {
  createRenderer,
  parseFrame,
  type RendererHandle,
} from "@/lib/renderer";

export interface Props {
  active: boolean;
  pollFrame: () => Promise<ArrayBuffer | null>;
  onCursor?: (c: { x: number; y: number; visible: boolean } | null) => void;
  onError?: (message: string) => void;
  presentation?: PresentationMode;
  cursorMode?: CursorMode;
}

export type SessionCanvasProps = Props;

const CURSOR_SVG =
  "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='18' height='18' viewBox='0 0 18 18'%3E%3Cpath d='M1 1l6.5 15.5 2.5-6 6-2.5L1 1z' fill='%23ffffff' stroke='%23000000' stroke-width='1.5' stroke-linejoin='round'/%3E%3C/svg%3E";

export function updateRemoteCursor(
  canvas: HTMLCanvasElement | null,
  cursorEl: HTMLElement | null,
  cx: number,
  cy: number,
  visible: boolean
): void {
  if (!cursorEl) return;
  if (!visible || cx < 0 || cy < 0 || !canvas) {
    cursorEl.style.display = "none";
    return;
  }
  const rect = canvasDisplayRect(canvas);
  if (rect.width <= 0 || rect.height <= 0) return;
  const px = rect.left + cx * rect.width;
  const py = rect.top + cy * rect.height;
  cursorEl.style.display = "block";
  cursorEl.style.transform = `translate(${px}px, ${py}px)`;
}

export function SessionCanvas({
  active,
  pollFrame,
  onCursor,
  onError,
  presentation = "fit",
  cursorMode = "remote",
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const cursorRef = useRef<HTMLDivElement | null>(null);
  const rendererRef = useRef<RendererHandle | null>(null);

  const pollFrameRef = useRef(pollFrame);
  const onCursorRef = useRef(onCursor);
  const onErrorRef = useRef(onError);
  const presentationRef = useRef(presentation);
  const cursorModeRef = useRef(cursorMode);
  presentationRef.current = presentation;
  cursorModeRef.current = cursorMode;
  const panRef = useRef({ x: 0.5, y: 0.5 });

  const sizeCanvas = () => {
    const canvas = canvasRef.current;
    const viewport = canvas?.parentElement;
    if (!canvas || !viewport || !viewport.clientWidth || !viewport.clientHeight) return;
    const rect = displayRect(viewport.clientWidth, viewport.clientHeight, canvas.width || 1920, canvas.height || 1080, presentationRef.current, panRef.current.x, panRef.current.y);
    Object.assign(canvas.style, { position: "absolute", maxWidth: "none", maxHeight: "none", width: `${rect.width}px`, height: `${rect.height}px`, left: `${rect.left}px`, top: `${rect.top}px` });
  };

  useEffect(() => {
    sizeCanvas();
    if (typeof ResizeObserver === "undefined" || !canvasRef.current?.parentElement) return;
    const observer = new ResizeObserver(sizeCanvas);
    observer.observe(canvasRef.current.parentElement);
    return () => observer.disconnect();
  }, [presentation]);

  useEffect(() => {
    pollFrameRef.current = pollFrame;
    onCursorRef.current = onCursor;
    onErrorRef.current = onError;
  });

  // THE CRITICAL INVARIANT:
  // The renderer is created EXACTLY ONCE per canvas mount.
  // Created in an effect that depends ONLY on mount ([]), never on props or state.
  // Re-renders caused by cursor updates or prop changes do not create another renderer.
  // On unmount, call dispose() exactly once.
  useEffect(() => {
    if (canvasRef.current) {
      try {
        rendererRef.current = createRenderer(canvasRef.current);
      } catch {
        rendererRef.current = null;
      }
      if (!rendererRef.current) {
        onErrorRef.current?.(
          "Video rendering is unavailable (WebGL context could not be created)"
        );
      }
    }
    return () => {
      if (rendererRef.current) {
        rendererRef.current.dispose();
        rendererRef.current = null;
      }
    };
  }, []);

  // Frame polling loop driven by requestAnimationFrame and guarded by generation counter
  const generationRef = useRef<number>(0);
  const rafRef = useRef<number | null>(null);
  const pollingRef = useRef(false);

  useEffect(() => {
    if (!active) {
      generationRef.current += 1;
      if (rafRef.current !== null) {
        const cancel =
          typeof cancelAnimationFrame === "function"
            ? cancelAnimationFrame
            : clearTimeout;
        cancel(rafRef.current);
        rafRef.current = null;
      }
      return;
    }

    const generation = ++generationRef.current;
    let running = true;

    const requestNext =
      typeof requestAnimationFrame === "function"
        ? requestAnimationFrame
        : (cb: FrameRequestCallback) =>
            setTimeout(() => cb(performance.now()), 16) as unknown as number;

    const cancelNext =
      typeof cancelAnimationFrame === "function"
        ? cancelAnimationFrame
        : (id: number) =>
            clearTimeout(id as unknown as ReturnType<typeof setTimeout>);

    const step = async () => {
      if (!running || generationRef.current !== generation) return;
      // Keep the display clock independent of IPC completion. A slow request
      // skips polling this refresh, not scheduling the next refresh.
      rafRef.current = requestNext(step);
      if (pollingRef.current) return;
      pollingRef.current = true;

      try {
        const buf = await pollFrameRef.current();
        if (!running || generationRef.current !== generation) return;

        if (buf && buf.byteLength >= 16) {
          // Parse exactly once per frame: the cursor projection and the
          // renderer share the same parsed result.
          const frame = parseFrame(buf);
          if (rendererRef.current) {
            if (frame) {
              rendererRef.current.render(frame);
            }
          } else {
            onErrorRef.current?.(
              "Video rendering is unavailable (WebGL context could not be created)"
            );
          }

          if (frame) {
            // The drawing buffer follows the actual stream size; hardcoded
            // attributes would be reapplied on every re-render and break
            // mouse mapping and cursor projection for other resolutions.
            const canvas = canvasRef.current;
            if (canvas && frame.width > 0 && frame.height > 0) {
              if (canvas.width !== frame.width) canvas.width = frame.width;
              if (canvas.height !== frame.height) canvas.height = frame.height;
              sizeCanvas();
            }
          }
          if (frame?.cursor) {
            updateRemoteCursor(
              canvasRef.current,
              cursorRef.current,
              frame.cursor.x,
              frame.cursor.y,
              frame.cursor.visible && cursorModeRef.current === "remote"
            );
            onCursorRef.current?.(frame.cursor);
          } else if (frame) {
            if (cursorRef.current) cursorRef.current.style.display = "none";
            onCursorRef.current?.(null);
          }
        }
      } catch (err: unknown) {
        if (!running || generationRef.current !== generation) return;
        const message = err instanceof Error ? err.message : String(err);
        onErrorRef.current?.(message);
      } finally {
        pollingRef.current = false;
      }
    };

    rafRef.current = requestNext(step);

    return () => {
      running = false;
      generationRef.current += 1;
      if (rafRef.current !== null) {
        cancelNext(rafRef.current);
        rafRef.current = null;
      }
    };
  }, [active]);

  return (
    <div
      id="viewport"
      tabIndex={0}
      className="relative w-full h-full flex items-center justify-center overflow-hidden"
      style={{ cursor: cursorMode === "local" ? "default" : "none" }}
      onMouseMove={(e) => {
        if (presentation !== "native") return;
        const rect = e.currentTarget.getBoundingClientRect();
        panRef.current = { x: Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width)), y: Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height)) };
        sizeCanvas();
      }}
    >
      <canvas
        id="video-canvas"
        ref={canvasRef}
        tabIndex={0}
        aria-label="Remote desktop video; focus to send remote input"
        className="pointer-events-auto"
      />
      <div
        id="remote-cursor"
        ref={cursorRef}
        className="remote-cursor"
        aria-hidden="true"
        style={{
          position: "fixed",
          width: "18px",
          height: "18px",
          pointerEvents: "none",
          display: "none",
          zIndex: 1005,
          backgroundImage: `url("${CURSOR_SVG}")`,
          backgroundRepeat: "no-repeat",
          backgroundSize: "contain",
          top: 0,
          left: 0,
          transform: "translate(-100px, -100px)",
          filter: "drop-shadow(0 1px 2px rgba(0, 0, 0, 0.5))",
        }}
      />
    </div>
  );
}
