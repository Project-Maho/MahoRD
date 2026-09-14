import { useEffect, useRef, useCallback, type RefObject } from "react";
import { sendInput, type InputPayload } from "@/lib/ipc";
import {
  createHeldInputTracker,
  shouldForwardKeyboardEvent,
  type MouseButton,
} from "@/lib/overlay";

export interface UseRemoteInputOptions {
  isConnected?: boolean;
  viewport?: HTMLElement | RefObject<HTMLElement | null> | null;
  canvas?: HTMLCanvasElement | RefObject<HTMLCanvasElement | null> | null;
  overlay?: HTMLElement | RefObject<HTMLElement | null> | null;
  onEscape?: () => void;
}

export interface RemoteInputHandle {
  (): Promise<void>;
  releaseAll: () => Promise<void>;
}

function resolveElement<T extends HTMLElement>(
  target?: T | RefObject<T | null> | null,
  fallbackIds?: string[]
): T | null {
  if (target) {
    if ("current" in target) {
      return target.current;
    }
    return target;
  }
  if (typeof document === "undefined" || !fallbackIds) return null;
  for (const id of fallbackIds) {
    const el = document.getElementById(id);
    if (el) return el as T;
  }
  return null;
}

function mapMouseButton(button: number): MouseButton | null {
  if (button === 0) return "left";
  if (button === 1) return "middle";
  if (button === 2) return "right";
  return null;
}

function mouseButtonEventType(button: MouseButton, isDown: boolean): string {
  if (button === "left") return isDown ? "LeftMouseDown" : "LeftMouseUp";
  if (button === "middle") return isDown ? "MiddleMouseDown" : "MiddleMouseUp";
  if (button === "right") return isDown ? "RightMouseDown" : "RightMouseUp";
  return "";
}

/**
 * Port of legacy input forwarding handlers from ui/index.html:
 *  - lines 1352-1361: viewport mousedown
 *  - lines 1363-1370: window mouseup
 *  - lines 1374-1397: release-all triggers (overlay pointerdown/focusin, window blur, doc visibilitychange)
 *  - lines 1399-1409: viewport mousemove
 *  - line  1411:      viewport contextmenu
 *  - lines 1413-1432: viewport wheel
 *  - lines 1434-1489: window keydown/keyup
 */
export function useRemoteInput(
  optionsOrConnected: UseRemoteInputOptions | boolean = true
): RemoteInputHandle {
  const options: UseRemoteInputOptions =
    typeof optionsOrConnected === "boolean"
      ? { isConnected: optionsOrConnected }
      : optionsOrConnected;

  const isConnected = options.isConnected ?? true;
  const isConnectedRef = useRef(isConnected);
  useEffect(() => {
    isConnectedRef.current = isConnected;
  }, [isConnected]);

  const optionsRef = useRef(options);
  useEffect(() => {
    optionsRef.current = options;
  });

  const heldInputsRef = useRef(createHeldInputTracker());
  const lastPointerRef = useRef({ x: 0, y: 0, viewWidth: 1280, viewHeight: 800 });
  const pendingPointerRef = useRef<InputPayload | null>(null);
  const pointerRafRef = useRef<number | null>(null);

  const flushPointerMotion = useCallback(() => {
    if (pointerRafRef.current !== null) {
      const cancel =
        typeof cancelAnimationFrame === "function"
          ? cancelAnimationFrame
          : clearTimeout;
      cancel(pointerRafRef.current);
      pointerRafRef.current = null;
    }
    const event = pendingPointerRef.current;
    pendingPointerRef.current = null;
    if (isConnectedRef.current && event) {
      sendInput(event).catch((err) => {
        console.error("send_input pointer error:", err);
      });
    }
  }, []);

  const releaseAll = useCallback(async (): Promise<void> => {
    flushPointerMotion();
    const canvas = resolveElement<HTMLCanvasElement>(
      optionsRef.current.canvas,
      ["video-canvas", "screen-canvas"]
    );
    const viewWidth =
      lastPointerRef.current.viewWidth || canvas?.clientWidth || 1280;
    const viewHeight =
      lastPointerRef.current.viewHeight || canvas?.clientHeight || 800;

    const events = heldInputsRef.current.releaseEvents(
      lastPointerRef.current.x,
      lastPointerRef.current.y,
      viewWidth,
      viewHeight
    );
    if (!events.length) return;

    for (const event of events) {
      try {
        await sendInput(event as InputPayload);
      } catch (err) {
        console.error("send_input release error:", err);
      }
    }
  }, [flushPointerMotion]);

  useEffect(() => {
    const viewport = resolveElement<HTMLElement>(
      optionsRef.current.viewport,
      ["viewport"]
    );
    const canvas = resolveElement<HTMLCanvasElement>(
      optionsRef.current.canvas,
      ["video-canvas", "screen-canvas"]
    );
    const overlay = resolveElement<HTMLElement>(
      optionsRef.current.overlay,
      ["session-overlay"]
    );

    const sendPointerEvent = (eventType: string, e: MouseEvent) => {
      if (!isConnectedRef.current) return;
      const isMotion =
        eventType === "MouseMove" ||
        eventType === "LeftMouseDragged" ||
        eventType === "RightMouseDragged";
      if (!isMotion) flushPointerMotion();

      const rect = canvas?.getBoundingClientRect?.();
      if (!rect || rect.width <= 0 || rect.height <= 0) return;

      const canvasWidth = canvas?.width || 1920;
      const canvasHeight = canvas?.height || 1080;
      const videoAspect =
        canvasWidth > 0 && canvasHeight > 0
          ? canvasWidth / canvasHeight
          : 16 / 9;
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

      const rawX = e.clientX - rect.left - offsetX;
      const rawY = e.clientY - rect.top - offsetY;
      const clampedX = Math.max(0, Math.min(rawX, displayW));
      const clampedY = Math.max(0, Math.min(rawY, displayH));

      lastPointerRef.current.x = clampedX;
      lastPointerRef.current.y = clampedY;
      lastPointerRef.current.viewWidth = displayW;
      lastPointerRef.current.viewHeight = displayH;

      let mod = 0;
      if (e.shiftKey) mod |= 1;
      if (e.ctrlKey) mod |= 2;
      if (e.altKey) mod |= 4;
      if (e.metaKey) mod |= 8;

      const event: InputPayload = {
        event_type: eventType,
        x: clampedX,
        y: clampedY,
        view_width: displayW,
        view_height: displayH,
        modifiers: mod,
        scroll_dx: 0.0,
        scroll_dy: 0.0,
      };

      if (isMotion) {
        pendingPointerRef.current = event;
        if (pointerRafRef.current === null) {
          const req =
            typeof requestAnimationFrame === "function"
              ? requestAnimationFrame
              : (cb: FrameRequestCallback) =>
                  setTimeout(() => cb(Date.now()), 16) as unknown as number;
          pointerRafRef.current = req(flushPointerMotion);
        }
        return;
      }

      sendInput(event).catch((err) => {
        console.error("send_input pointer error:", err);
      });
    };

    const sendRelativePointerEvent = (e: MouseEvent) => {
      if (!isConnectedRef.current) return;
      const dx = typeof e.movementX === "number" ? e.movementX : 0;
      const dy = typeof e.movementY === "number" ? e.movementY : 0;
      if (dx === 0 && dy === 0) return;

      let mod = 0;
      if (e.shiftKey) mod |= 1;
      if (e.ctrlKey) mod |= 2;
      if (e.altKey) mod |= 4;
      if (e.metaKey) mod |= 8;

      const event: InputPayload = {
        event_type: "RelativeMove",
        x: lastPointerRef.current.x,
        y: lastPointerRef.current.y,
        view_width: lastPointerRef.current.viewWidth || 1280,
        view_height: lastPointerRef.current.viewHeight || 800,
        modifiers: mod,
        scroll_dx: dx,
        scroll_dy: dy,
      };

      if (
        pendingPointerRef.current &&
        pendingPointerRef.current.event_type === "RelativeMove"
      ) {
        pendingPointerRef.current.scroll_dx =
          (pendingPointerRef.current.scroll_dx || 0) + dx;
        pendingPointerRef.current.scroll_dy =
          (pendingPointerRef.current.scroll_dy || 0) + dy;
        pendingPointerRef.current.modifiers = mod;
      } else {
        flushPointerMotion();
        pendingPointerRef.current = event;
        if (pointerRafRef.current === null) {
          const req =
            typeof requestAnimationFrame === "function"
              ? requestAnimationFrame
              : (cb: FrameRequestCallback) =>
                  setTimeout(() => cb(Date.now()), 16) as unknown as number;
          pointerRafRef.current = req(flushPointerMotion);
        }
      }
    };

    const releasePointerLock = () => {
      if (typeof document !== "undefined" && document.pointerLockElement) {
        try {
          document.exitPointerLock();
        } catch (_) {}
      }
    };

    const releaseLocalInputs = () => {
      if (!isConnectedRef.current) return;
      releaseAll().catch((err) => {
        console.error("releaseLocalInputs error:", err);
      });
    };

    // lines 1352-1361 viewport mousedown
    const handleMouseDown = (e: MouseEvent) => {
      if (!isConnectedRef.current) return;
      viewport?.focus?.();
      if (canvas && canvas.id !== "video-canvas") {
        canvas.focus?.();
      }
      const button = mapMouseButton(e.button);
      if (!button) return;
      if (e.button === 1) e.preventDefault?.();
      heldInputsRef.current.mouseDown(button);
      sendPointerEvent(mouseButtonEventType(button, true), e);
    };

    // lines 1363-1370 window mouseup
    const handleWindowMouseUp = (e: MouseEvent) => {
      const button = mapMouseButton(e.button);
      if (!button) return;
      if (heldInputsRef.current.isButtonDown(button)) {
        heldInputsRef.current.mouseUp(button);
        sendPointerEvent(mouseButtonEventType(button, false), e);
      }
    };

    // lines 1374-1397 release-all triggers
    const handleOverlayPointerDown = (e: PointerEvent) => {
      const target = e.target as Element | null;
      if (
        target &&
        typeof target.closest === "function" &&
        (target.closest("#btn-home") || target.closest("#btn-disconnect"))
      ) {
        return;
      }
      releaseLocalInputs();
    };

    const handleOverlayFocusIn = () => {
      releaseLocalInputs();
    };

    const handleBlur = () => {
      releasePointerLock();
      releaseLocalInputs();
    };

    const handleVisibilityChange = () => {
      if (typeof document !== "undefined" && document.hidden) {
        releasePointerLock();
        releaseLocalInputs();
      }
    };

    // lines 1399-1409 viewport mousemove
    // Bound at window level so a drag that leaves the canvas keeps its moves:
    // hover motion is still scoped to the viewport, held buttons and pointer
    // lock capture motion anywhere on the window.
    const isWithinViewport = (target: unknown): boolean => {
      if (!target) return false;
      if (target === viewport || target === canvas) return true;
      if (viewport && typeof viewport.contains === "function") {
        return viewport.contains(target as Node);
      }
      return false;
    };

    const handleMouseMove = (e: MouseEvent) => {
      const isLocked =
        typeof document !== "undefined" &&
        (document.pointerLockElement === canvas ||
          document.pointerLockElement === viewport ||
          Boolean(document.pointerLockElement));
      const leftDown = heldInputsRef.current.isButtonDown("left");
      const rightDown = heldInputsRef.current.isButtonDown("right");
      const dragging =
        leftDown || rightDown || heldInputsRef.current.isButtonDown("middle");
      if (!isLocked && !dragging && !isWithinViewport(e.target)) return;
      if (isLocked) {
        sendRelativePointerEvent(e);
      } else if (leftDown) {
        sendPointerEvent("LeftMouseDragged", e);
      } else if (rightDown) {
        sendPointerEvent("RightMouseDragged", e);
      } else {
        sendPointerEvent("MouseMove", e);
      }
    };

    // line 1411 viewport contextmenu
    const handleContextMenu = (e: MouseEvent) => {
      e.preventDefault?.();
    };

    // lines 1413-1432 viewport wheel
    const handleWheel = (e: WheelEvent) => {
      if (!isConnectedRef.current) return;
      e.preventDefault?.();
      const rect = canvas?.getBoundingClientRect?.() ?? {
        left: 0,
        top: 0,
        width: 1280,
        height: 800,
      };
      const width = rect.width > 0 ? rect.width : 1280;
      const height = rect.height > 0 ? rect.height : 800;
      const rawX = Math.max(0, Math.min(e.clientX - rect.left, width));
      const rawY = Math.max(0, Math.min(e.clientY - rect.top, height));

      sendInput({
        event_type: "ScrollWheel",
        x: rawX,
        y: rawY,
        view_width: width,
        view_height: height,
        modifiers: 0,
        scroll_dx: e.deltaX,
        scroll_dy: e.deltaY,
      }).catch((err) => {
        console.error("send_input wheel error:", err);
      });
    };

    // lines 1434-1489 window keydown/keyup
    const handleKeyDown = (e: KeyboardEvent) => {
      const target = e.target as { tagName?: unknown; id?: unknown } | null;
      if (!shouldForwardKeyboardEvent({ target })) {
        if (e.key === "Escape") {
          optionsRef.current.onEscape?.();
        }
        return;
      }
      if (!isConnectedRef.current) return;
      // Forwarded keys belong to the remote host: Tab must not move local focus
      // and browser/webview shortcuts must not navigate the shell.
      e.preventDefault?.();

      let mod = 0;
      if (e.shiftKey) mod |= 1;
      if (e.ctrlKey) mod |= 2;
      if (e.altKey) mod |= 4;
      if (e.metaKey) mod |= 8;

      const keyCode =
        (e.keyCode !== undefined ? e.keyCode : (e.which ?? 0)) || 0;
      heldInputsRef.current.keyDown(keyCode, mod);

      const viewWidth =
        canvas?.clientWidth || lastPointerRef.current.viewWidth || 1280;
      const viewHeight =
        canvas?.clientHeight || lastPointerRef.current.viewHeight || 800;

      sendInput({
        event_type: "KeyDown",
        key_code: keyCode,
        modifiers: mod,
        view_width: viewWidth,
        view_height: viewHeight,
      }).catch((err) => {
        console.error("send_input keydown error:", err);
      });
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      const target = e.target as { tagName?: unknown; id?: unknown } | null;
      if (!shouldForwardKeyboardEvent({ target })) return;
      if (!isConnectedRef.current) return;
      e.preventDefault?.();

      let mod = 0;
      if (e.shiftKey) mod |= 1;
      if (e.ctrlKey) mod |= 2;
      if (e.altKey) mod |= 4;
      if (e.metaKey) mod |= 8;

      const keyCode =
        (e.keyCode !== undefined ? e.keyCode : (e.which ?? 0)) || 0;
      heldInputsRef.current.keyUp(keyCode);

      const viewWidth =
        canvas?.clientWidth || lastPointerRef.current.viewWidth || 1280;
      const viewHeight =
        canvas?.clientHeight || lastPointerRef.current.viewHeight || 800;

      sendInput({
        event_type: "KeyUp",
        key_code: keyCode,
        modifiers: mod,
        view_width: viewWidth,
        view_height: viewHeight,
      }).catch((err) => {
        console.error("send_input keyup error:", err);
      });
    };

    if (viewport) {
      viewport.addEventListener("mousedown", handleMouseDown as EventListener);
      viewport.addEventListener("contextmenu", handleContextMenu as EventListener);
      viewport.addEventListener("wheel", handleWheel as EventListener, {
        passive: false,
      });
    }

    if (overlay) {
      overlay.addEventListener(
        "pointerdown",
        handleOverlayPointerDown as EventListener
      );
      overlay.addEventListener("focusin", handleOverlayFocusIn as EventListener);
    }

    if (typeof window !== "undefined") {
      window.addEventListener("mousemove", handleMouseMove as EventListener);
      window.addEventListener("mouseup", handleWindowMouseUp as EventListener);
      window.addEventListener("keydown", handleKeyDown as EventListener);
      window.addEventListener("keyup", handleKeyUp as EventListener);
      window.addEventListener("blur", handleBlur as EventListener);
    }

    if (typeof document !== "undefined") {
      document.addEventListener(
        "visibilitychange",
        handleVisibilityChange as EventListener
      );
    }

    return () => {
      if (viewport) {
        viewport.removeEventListener(
          "mousedown",
          handleMouseDown as EventListener
        );
        viewport.removeEventListener(
          "contextmenu",
          handleContextMenu as EventListener
        );
        viewport.removeEventListener("wheel", handleWheel as EventListener);
      }

      if (overlay) {
        overlay.removeEventListener(
          "pointerdown",
          handleOverlayPointerDown as EventListener
        );
        overlay.removeEventListener(
          "focusin",
          handleOverlayFocusIn as EventListener
        );
      }

      if (typeof window !== "undefined") {
        window.removeEventListener(
          "mousemove",
          handleMouseMove as EventListener
        );
        window.removeEventListener(
          "mouseup",
          handleWindowMouseUp as EventListener
        );
        window.removeEventListener("keydown", handleKeyDown as EventListener);
        window.removeEventListener("keyup", handleKeyUp as EventListener);
        window.removeEventListener("blur", handleBlur as EventListener);
      }

      if (typeof document !== "undefined") {
        document.removeEventListener(
          "visibilitychange",
          handleVisibilityChange as EventListener
        );
      }

      // Teardown: flush motion and release all held inputs so nothing stays stuck on the host
      flushPointerMotion();
      releaseAll().catch(() => {});
    };
    // The elements are resolved when the effect runs, so the dependencies must
    // include everything that can change which elements those are: a new
    // target/ref from the caller, or a connection transition that remounts the
    // session DOM. Re-running teardown also releases held inputs on disconnect
    // instead of orphaning them until unmount.
  }, [
    releaseAll,
    flushPointerMotion,
    isConnected,
    options.viewport,
    options.canvas,
    options.overlay,
  ]);

  const handle = useCallback(() => releaseAll(), [releaseAll]) as RemoteInputHandle;
  handle.releaseAll = releaseAll;
  return handle;
}
