export type PresentationMode = "fit" | "stretch" | "native";
export type CursorMode = "local" | "remote" | "hidden";
export interface DisplayRect {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

export function displayRect(width: number, height: number, videoWidth: number, videoHeight: number, mode: PresentationMode, panX = 0.5, panY = 0.5): DisplayRect {
  const scale = mode === "native" ? 1 : Math.min(width / videoWidth, height / videoHeight);
  const w = mode === "stretch" ? width : videoWidth * scale;
  const h = mode === "stretch" ? height : videoHeight * scale;
  return { left: (width - w) * (w > width ? panX : 0.5), top: (height - h) * (h > height ? panY : 0.5), width: w, height: h };
}

// The canvas CSS box is the video rectangle, shared by cursor and input projection.
export function canvasDisplayRect(canvas: HTMLCanvasElement): DisplayRect {
  const rect = canvas.getBoundingClientRect();
  if (canvas.style?.position !== "absolute") {
    const fitted = displayRect(rect.width, rect.height, canvas.width || 1920, canvas.height || 1080, "fit");
    return { ...fitted, left: rect.left + fitted.left, top: rect.top + fitted.top };
  }
  return { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
}

export function normalizeWheel(delta: number, mode: number, pagePixels: number, sensitivity: number): number {
  return delta * (mode === 1 ? 16 : mode === 2 ? pagePixels : 1) * sensitivity;
}

export function sessionShortcut(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "shiftKey" | "altKey" | "metaKey">): "fullscreen" | "overlay" | "keyboard" | null {
  // Ctrl is intentional on macOS too: Cmd is already part of the remote input path.
  if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return null;
  switch (e.key?.toLowerCase()) {
    case "f": return "fullscreen";
    case "m": return "overlay";
    case "z": return "keyboard";
    default: return null;
  }
}

export const SCROLL_STORAGE_KEY = "maho-scroll-sensitivity";
export function toolbarPinned(expanded: boolean, hovered: boolean, focused: boolean, connecting: boolean, confirming: boolean): boolean {
  return expanded || hovered || focused || connecting || confirming;
}
export function savedScrollSensitivity(): number {
  if (typeof window === "undefined" || !window.localStorage) return 1;
  const value = Number(window.localStorage.getItem(SCROLL_STORAGE_KEY));
  return [0.25, 0.5, 1, 2, 4].includes(value) ? value : 1;
}
