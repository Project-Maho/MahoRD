import { test, expect } from "bun:test";
import {
  isRemoteInputTarget,
  shouldForwardKeyboardEvent,
  createHeldInputTracker,
  createOverlayState,
  normalizeMouseButton,
  buttonUpType,
  type KeyReleaseEvent,
} from "./overlay";

// ---------------------------------------------------------------------------
// 1. Input-leak regression: keyboard events targeting overlay UI (buttons,
//    panel, connecting modal, dashboard chrome, inputs) must never be
//    forwarded to the remote host.
// ---------------------------------------------------------------------------
test("keyboard events on overlay UI are not forwarded to the remote host", () => {
  const button = { tagName: "BUTTON", id: "" };
  const panelStat = { tagName: "DD", id: "session-stat-state" };
  const input = { tagName: "INPUT", id: "direct-ip" };
  const modalButton = { tagName: "BUTTON", id: "" };
  expect(shouldForwardKeyboardEvent({ target: button })).toBe(false);
  expect(shouldForwardKeyboardEvent({ target: panelStat })).toBe(false);
  expect(shouldForwardKeyboardEvent({ target: input })).toBe(false);
  expect(shouldForwardKeyboardEvent({ target: modalButton })).toBe(false);
  expect(shouldForwardKeyboardEvent({ target: null })).toBe(false);
  expect(shouldForwardKeyboardEvent(undefined)).toBe(false);
});

test("keyboard events on the video surface are forwarded to the remote host", () => {
  expect(shouldForwardKeyboardEvent({ target: { tagName: "DIV", id: "viewport" } })).toBe(true);
  expect(shouldForwardKeyboardEvent({ target: { tagName: "DIV", id: "viewport-container" } })).toBe(true);
  expect(shouldForwardKeyboardEvent({ target: { tagName: "CANVAS", id: "screen-canvas" } })).toBe(true);
  expect(shouldForwardKeyboardEvent({ target: { tagName: "CANVAS", id: "video-canvas" } })).toBe(true);
});

// ---------------------------------------------------------------------------
// 2. Safe release of held input: every key/button the UI pressed into the
//    remote must produce an explicit release event when attention moves to
//    the overlay (or the window loses focus), and releasing must be
//    idempotent.
// ---------------------------------------------------------------------------
test("held input tracker is empty by default and tracks key state", () => {
  const t = createHeldInputTracker();
  expect(t.size).toBe(0);
  expect(t.isKeyDown(0x41)).toBe(false);

  t.keyDown(0x41, 1);
  expect(t.isKeyDown(0x41)).toBe(true);
  expect(t.size).toBe(1);

  t.keyUp(0x41);
  expect(t.isKeyDown(0x41)).toBe(false);
  expect(t.size).toBe(0);
});

test("releaseEvents emits mouse-ups then key-ups and clears state", () => {
  const t = createHeldInputTracker();
  t.mouseDown("left");
  t.mouseDown("right");
  t.keyDown(0x41, 1); // 'A' with shift
  t.keyDown(0x2e, 0);
  expect(t.size).toBe(4);

  const events = t.releaseEvents(320.5, 240.25, 1280, 800);

  expect(events.length).toBe(4);
  // Buttons release first (mirrors the Rust InputStateTracker::release_all order).
  expect(events.map((e) => e.event_type)).toEqual([
    "LeftMouseUp",
    "RightMouseUp",
    "KeyUp",
    "KeyUp",
  ]);
  expect(events[0]).toEqual({
    event_type: "LeftMouseUp",
    x: 320.5,
    y: 240.25,
    view_width: 1280,
    view_height: 800,
  });
  const keyA = events[2] as KeyReleaseEvent;
  expect(keyA.key_code).toBe(0x41);
  expect(keyA.modifiers).toBe(1);
  const keyDot = events[3] as KeyReleaseEvent;
  expect(keyDot.key_code).toBe(0x2e);

  // Releasing again must be a no-op (idempotent, no duplicate ups).
  expect(t.releaseEvents(0, 0, 1280, 800)).toEqual([]);
  expect(t.size).toBe(0);
});

test("releasing a key before releaseEvents drops it from the release batch", () => {
  const t = createHeldInputTracker();
  t.keyDown(0x41, 0);
  t.keyDown(0x42, 0);
  t.keyUp(0x41);
  const events = t.releaseEvents(0, 0, 1280, 800);
  expect(events.length).toBe(1);
  expect((events[0] as KeyReleaseEvent).key_code).toBe(0x42);
});

test("mouse button up only releases buttons that were pressed", () => {
  const t = createHeldInputTracker();
  t.mouseDown("left");
  expect(t.isButtonDown("left")).toBe(true);
  expect(t.isButtonDown("right")).toBe(false);
  t.mouseUp("left");
  t.mouseUp("right"); // spurious up: ignored
  expect(t.size).toBe(0);
  expect(t.releaseEvents(0, 0, 1280, 800)).toEqual([]);
});

test("held input tracker tracks middle button and emits MiddleMouseUp", () => {
  const t = createHeldInputTracker();
  t.mouseDown("middle");
  expect(t.isButtonDown("middle")).toBe(true);
  expect(t.isButtonDown("left")).toBe(false);
  expect(t.isButtonDown("right")).toBe(false);
  const events = t.releaseEvents(100, 200, 1280, 800);
  expect(events.length).toBe(1);
  expect(events[0]).toEqual({
    event_type: "MiddleMouseUp",
    x: 100,
    y: 200,
    view_width: 1280,
    view_height: 800,
  });
  expect(t.size).toBe(0);
});

test("unsupported extra buttons never map to left button", () => {
  const t = createHeldInputTracker();
  t.mouseDown(3);
  t.mouseDown(4);
  t.mouseDown("extra");
  t.mouseDown("back");
  expect(t.isButtonDown("left")).toBe(false);
  expect(t.isButtonDown("middle")).toBe(false);
  expect(t.isButtonDown("right")).toBe(false);
  expect(t.size).toBe(0);
  expect(t.releaseEvents(0, 0, 1280, 800)).toEqual([]);
});

test("releaseEvents releases all held buttons in order", () => {
  const t = createHeldInputTracker();
  t.mouseDown("left");
  t.mouseDown("middle");
  t.mouseDown("right");
  expect(t.size).toBe(3);
  const events = t.releaseEvents(10, 20, 1280, 800);
  expect(events.length).toBe(3);
  expect(events.map((e) => e.event_type)).toEqual([
    "LeftMouseUp",
    "MiddleMouseUp",
    "RightMouseUp",
  ]);
  expect(t.size).toBe(0);
});

// ---------------------------------------------------------------------------
// 3. Overlay UI state: expand/collapse, fullscreen, and pointer lock state
// ---------------------------------------------------------------------------
test("overlay state toggles expansion and notifies subscribers", () => {
  const s = createOverlayState();
  expect(s.getExpanded()).toBe(false);

  const seen: boolean[] = [];
  const unsubscribe = s.subscribe((state) => seen.push(state.expanded));

  s.toggleExpanded();
  expect(s.getExpanded()).toBe(true);
  s.toggleExpanded();
  expect(s.getExpanded()).toBe(false);
  expect(seen).toEqual([true, false]);

  unsubscribe();
  s.toggleExpanded();
  expect(seen).toEqual([true, false]);
});

test("overlay state tracks fullscreen pressed-state", () => {
  const s = createOverlayState();
  expect(s.getFullscreenActive()).toBe(false);
  s.setFullscreenActive(true);
  expect(s.getFullscreenActive()).toBe(true);
  s.setFullscreenActive(false);
  expect(s.getFullscreenActive()).toBe(false);
});

test("overlay state tracks pointer lock active state", () => {
  const s = createOverlayState();
  expect(typeof s.getPointerLockActive).toBe("function");
  expect(s.getPointerLockActive()).toBe(false);
  s.setPointerLockActive(true);
  expect(s.getPointerLockActive()).toBe(true);
  s.setPointerLockActive(false);
  expect(s.getPointerLockActive()).toBe(false);
});

// ---------------------------------------------------------------------------
// 4. Helper unit tests: normalizeMouseButton and buttonUpType
// ---------------------------------------------------------------------------
test("normalizeMouseButton correctly maps valid buttons and returns null for invalid", () => {
  expect(normalizeMouseButton("left")).toBe("left");
  expect(normalizeMouseButton(0)).toBe("left");
  expect(normalizeMouseButton("middle")).toBe("middle");
  expect(normalizeMouseButton(1)).toBe("middle");
  expect(normalizeMouseButton("right")).toBe("right");
  expect(normalizeMouseButton(2)).toBe("right");
  expect(normalizeMouseButton(3)).toBe(null);
  expect(normalizeMouseButton("unknown")).toBe(null);
  expect(normalizeMouseButton(undefined)).toBe(null);
});

test("buttonUpType returns matching wire event type", () => {
  expect(buttonUpType("left")).toBe("LeftMouseUp");
  expect(buttonUpType("middle")).toBe("MiddleMouseUp");
  expect(buttonUpType("right")).toBe("RightMouseUp");
  // MouseEvent.button numeric codes are accepted like normalizeMouseButton.
  expect(buttonUpType(0)).toBe("LeftMouseUp");
  expect(buttonUpType(1)).toBe("MiddleMouseUp");
  expect(buttonUpType(2)).toBe("RightMouseUp");
  // Extra buttons must never map to a left click.
  expect(buttonUpType(3)).toBe(null);
  expect(buttonUpType("other")).toBe(null);
  expect(buttonUpType(null)).toBe(null);
});

test("isRemoteInputTarget rejects invalid targets and identifies remote surfaces", () => {
  expect(isRemoteInputTarget(null)).toBe(false);
  expect(isRemoteInputTarget(undefined)).toBe(false);
  expect(isRemoteInputTarget({})).toBe(false);
  // Focus on document.body is local UI attention, never the remote surface:
  // keystrokes must stay in the shell instead of leaking to the host.
  expect(isRemoteInputTarget({ tagName: "BODY" })).toBe(false);
  expect(isRemoteInputTarget({ tagName: "BODY", id: "" })).toBe(false);
  expect(isRemoteInputTarget({ tagName: "DIV", id: "viewport" })).toBe(true);
  expect(isRemoteInputTarget({ tagName: "DIV", id: "viewport-container" })).toBe(true);
  expect(isRemoteInputTarget({ tagName: "CANVAS", id: "screen-canvas" })).toBe(true);
  expect(isRemoteInputTarget({ tagName: "CANVAS", id: "video-canvas" })).toBe(true);
  expect(isRemoteInputTarget({ tagName: "DIV", id: "other" })).toBe(false);
});
