import { useEffect, useRef } from "react";
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
  const rect = canvas.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return;
  const videoAspect =
    canvas.width > 0 && canvas.height > 0 ? canvas.width / canvas.height : 16 / 9;
  const canvasAspect = rect.width / rect.height;
  let displayW = rect.width;
  let displayH = rect.height;
  let offsetX = 0;
  let offsetY = 0;

  if (canvasAspect > videoAspect) {
    displayH = rect.height;
    displayW = displayH * videoAspect;
    offsetX = (rect.width - displayW) / 2;
  } else {
    displayW = rect.width;
    displayH = displayW / videoAspect;
    offsetY = (rect.height - displayH) / 2;
  }

  const px = rect.left + offsetX + cx * displayW;
  const py = rect.top + offsetY + cy * displayH;
  cursorEl.style.display = "block";
  cursorEl.style.transform = `translate(${px}px, ${py}px)`;
}

export function SessionCanvas({
  active,
  pollFrame,
  onCursor,
  onError,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const cursorRef = useRef<HTMLDivElement | null>(null);
  const rendererRef = useRef<RendererHandle | null>(null);

  const pollFrameRef = useRef(pollFrame);
  const onCursorRef = useRef(onCursor);
  const onErrorRef = useRef(onError);

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
      rendererRef.current = createRenderer(canvasRef.current);
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
      rafRef.current = null;

      try {
        const buf = await pollFrameRef.current();
        if (!running || generationRef.current !== generation) return;

        if (buf && buf.byteLength >= 16) {
          if (rendererRef.current) {
            rendererRef.current.render(buf);
          } else {
            onErrorRef.current?.(
              "Video rendering is unavailable (WebGL context could not be created)"
            );
          }

          const frame = parseFrame(buf);
          if (frame) {
            // The drawing buffer follows the actual stream size; hardcoded
            // attributes would be reapplied on every re-render and break
            // mouse mapping and cursor projection for other resolutions.
            const canvas = canvasRef.current;
            if (canvas && frame.width > 0 && frame.height > 0) {
              if (canvas.width !== frame.width) canvas.width = frame.width;
              if (canvas.height !== frame.height) canvas.height = frame.height;
            }
          }
          if (frame?.cursor) {
            updateRemoteCursor(
              canvasRef.current,
              cursorRef.current,
              frame.cursor.x,
              frame.cursor.y,
              frame.cursor.visible
            );
            onCursorRef.current?.(frame.cursor);
          } else if (frame) {
            onCursorRef.current?.(null);
          }
        }
      } catch (err: unknown) {
        if (!running || generationRef.current !== generation) return;
        const message = err instanceof Error ? err.message : String(err);
        onErrorRef.current?.(message);
      }

      if (!running || generationRef.current !== generation) return;
      rafRef.current = requestNext(step);
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
          position: "absolute",
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
