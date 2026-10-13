import { describe, expect, it } from "bun:test";
import { displayRect, normalizeWheel, sessionShortcut, toolbarPinned } from "./presentation";

describe("session presentation", () => {
  it("pins the toolbar for local attention or connection progress", () => {
    expect(toolbarPinned(false, false, false, false, false)).toBe(false);
    expect(toolbarPinned(true, false, false, false, false)).toBe(true);
    expect(toolbarPinned(false, true, false, false, false)).toBe(true);
    expect(toolbarPinned(false, false, true, false, false)).toBe(true);
    expect(toolbarPinned(false, false, false, true, false)).toBe(true);
  });
  it("fits by height without overflowing either viewport dimension", () => {
    expect(displayRect(1200, 600, 1920, 1080, "fit")).toEqual({ left: (1200 - 1920 * 600 / 1080) / 2, top: 0, width: 1920 * 600 / 1080, height: 600 });
  });
  it("stretches the shared video rectangle to the viewport", () => {
    expect(displayRect(1200, 600, 1920, 1080, "stretch")).toEqual({ left: 0, top: 0, width: 1200, height: 600 });
  });
  it("clips native pixels and pans to the far edge", () => {
    expect(displayRect(1200, 600, 1920, 1080, "native", 1, 1)).toEqual({ left: -720, top: -480, width: 1920, height: 1080 });
  });
  it("normalizes line wheel deltas before sensitivity", () => {
    expect(normalizeWheel(3, 1, 600, 2)).toBe(96);
  });
  it("normalizes page wheel deltas to viewport pixels", () => {
    expect(normalizeWheel(-1, 2, 600, 0.5)).toBe(-300);
  });
  it("preserves pixel wheel direction and units", () => {
    expect(normalizeWheel(100, 0, 600, 1)).toBe(100);
  });
  for (const [key, action] of [["f", "fullscreen"], ["m", "overlay"], ["z", "keyboard"]] as const) {
    it(`recognizes the local ${action} chord`, () => {
      expect(sessionShortcut({ key, ctrlKey: true, shiftKey: true, altKey: false, metaKey: false })).toBe(action);
    });
  }
  it("keeps Cmd chords in the existing remote input path", () => {
    expect(sessionShortcut({ key: "f", ctrlKey: false, shiftKey: true, altKey: false, metaKey: true })).toBeNull();
  });
});
