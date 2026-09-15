/**
 * Session overlay core logic for the MahoRD Tauri shell.
 *
 * Pure, DOM-free logic ported from ui/session-overlay.js as a strict
 * TypeScript ES module.
 *
 * Design contract (Parsec-style session overlay):
 *  - A compact launcher bar stays visible above the video at all times.
 *  - An expandable panel integrates stats + fullscreen + session actions.
 *  - Keyboard/pointer input must never leak from overlay UI to the remote
 *    host; only events targeting the video surface are forwarded.
 *  - Input held down on the video (keys / mouse buttons) is released
 *    explicitly when attention moves to local UI (mirrors the host-side
 *    `InputStateTracker::release_all` semantics: buttons first, then keys).
 */

/**
 * Target IDs on which keyboard input belongs to the remote host.
 */
const REMOTE_TARGET_IDS = new Set<string>(['viewport', 'viewport-container', 'screen-canvas', 'video-canvas']);

/**
 * Supported mouse button identifiers.
 */
export type MouseButton = 'left' | 'middle' | 'right';

/**
 * Wire event types for mouse button release.
 */
export type MouseUpEventType = 'LeftMouseUp' | 'MiddleMouseUp' | 'RightMouseUp';

/**
 * Wire event for mouse button release.
 */
export interface MouseReleaseEvent {
  event_type: MouseUpEventType;
  x: number;
  y: number;
  view_width: number;
  view_height: number;
}

/**
 * Wire event for key release.
 */
export interface KeyReleaseEvent {
  event_type: 'KeyUp';
  key_code: number;
  modifiers: number;
}

/**
 * Union of all release events produced by the held input tracker.
 */
export type ReleaseEvent = MouseReleaseEvent | KeyReleaseEvent;

/**
 * Minimal target representation for input targeting checks without DOM dependency.
 */
export interface InputTargetLike {
  tagName?: unknown;
  id?: unknown;
}

/**
 * Minimal keyboard event representation for forwarding checks.
 */
export interface KeyboardEventLike {
  target?: InputTargetLike | null;
}

/**
 * Normalizes mouse button names/indices to supported wire variants.
 * Unsupported extra buttons (e.g. 3, 4, back, forward) return null
 * and must never map to left click.
 */
export function normalizeMouseButton(button: unknown): MouseButton | null {
  if (button === 'left' || button === 0) return 'left';
  if (button === 'middle' || button === 1) return 'middle';
  if (button === 'right' || button === 2) return 'right';
  return null;
}

/**
 * Maps a mouse button identifier to its wire mouse-up event type.
 */
export function buttonUpType(button: unknown): MouseUpEventType | null {
  // Accept the same inputs as normalizeMouseButton, including MouseEvent.button
  // numeric codes (0/1/2).
  const norm = normalizeMouseButton(button);
  if (norm === 'left') return 'LeftMouseUp';
  if (norm === 'middle') return 'MiddleMouseUp';
  if (norm === 'right') return 'RightMouseUp';
  return null;
}

/**
 * True when a target element is part of the remote video surface (i.e. its
 * keyboard events should be forwarded to the host). Everything else —
 * overlay buttons, the launcher panel, the connecting modal, dashboard
 * chrome, text inputs — is local UI and must never receive forwarding.
 */
export function isRemoteInputTarget(target: unknown): boolean {
  if (!target || typeof (target as InputTargetLike).tagName !== 'string') {
    return false;
  }
  const el = target as InputTargetLike;
  // Focus sitting on document.body (clicks on non-input overlay chrome,
  // dismissed dropdowns, clicks outside modals) is local UI attention, never
  // the remote video surface: only explicit remote target IDs forward.
  return typeof el.id === 'string' && REMOTE_TARGET_IDS.has(el.id as string);
}

/**
 * Guard for window-level keydown/keyup forwarding.
 */
export function shouldForwardKeyboardEvent(event?: KeyboardEventLike | null): boolean {
  const target = event ? event.target : null;
  return isRemoteInputTarget(target);
}

/**
 * Tracker interface for input held down into the remote session.
 */
export interface HeldInputTracker {
  keyDown(keyCode: number, modifiers?: number): void;
  keyUp(keyCode: number): void;
  isKeyDown(keyCode: number): boolean;
  mouseDown(button: unknown): void;
  mouseUp(button: unknown): void;
  isButtonDown(button: unknown): boolean;
  readonly size: number;
  releaseEvents(x: number, y: number, viewWidth: number, viewHeight: number): ReleaseEvent[];
}

/**
 * Tracks input the UI pressed down into the remote session so it can be
 * released safely when the user moves attention to the overlay, the
 * window loses focus, or the session ends.
 */
export function createHeldInputTracker(): HeldInputTracker {
  const heldKeys = new Map<number, number>(); // keyCode -> modifiers captured at press time
  const heldButtons = new Set<MouseButton>(); // 'left' | 'middle' | 'right'

  return {
    keyDown(keyCode: number, modifiers?: number): void {
      heldKeys.set(keyCode, modifiers || 0);
    },
    keyUp(keyCode: number): void {
      heldKeys.delete(keyCode);
    },
    isKeyDown(keyCode: number): boolean {
      return heldKeys.has(keyCode);
    },
    mouseDown(button: unknown): void {
      const norm = normalizeMouseButton(button);
      if (norm) heldButtons.add(norm);
    },
    mouseUp(button: unknown): void {
      const norm = normalizeMouseButton(button);
      if (norm) heldButtons.delete(norm);
    },
    isButtonDown(button: unknown): boolean {
      const norm = normalizeMouseButton(button);
      return norm ? heldButtons.has(norm) : false;
    },
    get size(): number {
      return heldKeys.size + heldButtons.size;
    },
    /**
     * Builds send_input payloads releasing everything held: mouse buttons
     * first, then keys (same ordering as InputStateTracker::release_all).
     * Clears the tracker; a second call is a no-op.
     */
    releaseEvents(x: number, y: number, viewWidth: number, viewHeight: number): ReleaseEvent[] {
      const events: ReleaseEvent[] = [];
      for (const button of heldButtons) {
        const upType = buttonUpType(button);
        if (upType) {
          events.push({
            event_type: upType,
            x,
            y,
            view_width: viewWidth,
            view_height: viewHeight,
          });
        }
      }
      for (const [keyCode, modifiers] of heldKeys) {
        events.push({
          event_type: 'KeyUp',
          key_code: keyCode,
          modifiers,
        });
      }
      heldButtons.clear();
      heldKeys.clear();
      return events;
    },
  };
}

/**
 * State snapshot emitted to overlay listeners.
 */
export interface OverlaySnapshot {
  expanded: boolean;
  fullscreenActive: boolean;
  pointerLockActive: boolean;
}

/**
 * Initial configuration options for overlay state.
 */
export interface OverlayStateOptions {
  expanded?: boolean;
  fullscreenActive?: boolean;
  pointerLockActive?: boolean;
}

/**
 * Listener callback signature for overlay state changes.
 */
export type OverlayListener = (snapshot: OverlaySnapshot) => void;

/**
 * Observable overlay state interface.
 */
export interface OverlayState {
  getExpanded(): boolean;
  setExpanded(value: boolean): void;
  toggleExpanded(): void;
  getFullscreenActive(): boolean;
  setFullscreenActive(value: boolean): void;
  getPointerLockActive(): boolean;
  setPointerLockActive(value: boolean): void;
  subscribe(listener: OverlayListener): () => void;
}

/**
 * Small observable state for the launcher: expansion of the controls
 * panel, the fullscreen toggle's pressed state, and pointer lock state.
 */
export function createOverlayState(initial?: OverlayStateOptions): OverlayState {
  const options = initial || {};
  let expanded = !!options.expanded;
  let fullscreenActive = !!options.fullscreenActive;
  let pointerLockActive = !!options.pointerLockActive;
  const listeners = new Set<OverlayListener>();

  function emit(): void {
    const snapshot: OverlaySnapshot = {
      expanded,
      fullscreenActive,
      pointerLockActive,
    };
    for (const listener of listeners) {
      listener(snapshot);
    }
  }

  return {
    getExpanded(): boolean {
      return expanded;
    },
    setExpanded(value: boolean): void {
      expanded = !!value;
      emit();
    },
    toggleExpanded(): void {
      expanded = !expanded;
      emit();
    },
    getFullscreenActive(): boolean {
      return fullscreenActive;
    },
    setFullscreenActive(value: boolean): void {
      const next = !!value;
      if (next !== fullscreenActive) {
        fullscreenActive = next;
        emit();
      }
    },
    getPointerLockActive(): boolean {
      return pointerLockActive;
    },
    setPointerLockActive(value: boolean): void {
      const next = !!value;
      if (next !== pointerLockActive) {
        pointerLockActive = next;
        emit();
      }
    },
    subscribe(listener: OverlayListener): () => void {
      listeners.add(listener);
      return function unsubscribe(): void {
        listeners.delete(listener);
      };
    },
  };
}
