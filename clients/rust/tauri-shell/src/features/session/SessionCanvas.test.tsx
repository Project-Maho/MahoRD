import { describe, it, expect, mock, beforeEach } from "bun:test";

// Set up minimal DOM environment before importing react-dom/client if document is undefined
if (typeof globalThis.document === "undefined") {
  function createFakeElement(tag: string, ownerDoc: any): any {
    const el: any = {
      tagName: (tag || "div").toUpperCase(),
      nodeType: 1,
      childNodes: [] as any[],
      children: [] as any[],
      style: {} as Record<string, string>,
      attributes: {} as Record<string, string>,
      setAttribute(k: string, v: string) {
        this.attributes[k] = v;
      },
      getAttribute(k: string) {
        return this.attributes[k] ?? null;
      },
      removeAttribute(k: string) {
        delete this.attributes[k];
      },
      addEventListener() {},
      removeEventListener() {},
      dispatchEvent() {
        return true;
      },
      appendChild(c: any) {
        this.childNodes.push(c);
        if (c.nodeType === 1) this.children.push(c);
        c.parentNode = this;
        return c;
      },
      removeChild(c: any) {
        const i = this.childNodes.indexOf(c);
        if (i !== -1) this.childNodes.splice(i, 1);
        const j = this.children.indexOf(c);
        if (j !== -1) this.children.splice(j, 1);
        c.parentNode = null;
        return c;
      },
      insertBefore(n: any, ref: any) {
        const i = this.childNodes.indexOf(ref);
        if (i !== -1) this.childNodes.splice(i, 0, n);
        else this.childNodes.push(n);
        if (n.nodeType === 1) {
          const j = this.children.indexOf(ref);
          if (j !== -1) this.children.splice(j, 0, n);
          else this.children.push(n);
        }
        n.parentNode = this;
        return n;
      },
      ownerDocument: ownerDoc,
      parentNode: null,
      getBoundingClientRect() {
        return { left: 0, top: 0, width: 1920, height: 1080, right: 1920, bottom: 1080 };
      },
      getContext() {
        return {};
      },
    };
    return el;
  }

  const doc: any = {
    nodeType: 9,
    ownerDocument: null,
    createElement(tag: string) {
      return createFakeElement(tag, doc);
    },
    createElementNS(_ns: string, tag: string) {
      return createFakeElement(tag, doc);
    },
    createTextNode(text: string) {
      return { nodeType: 3, textContent: text, ownerDocument: doc, parentNode: null };
    },
    createComment(data: string) {
      return { nodeType: 8, data, ownerDocument: doc, parentNode: null };
    },
    createDocumentFragment() {
      const f = createFakeElement("#document-fragment", doc);
      f.nodeType = 11;
      return f;
    },
    addEventListener() {},
    removeEventListener() {},
    dispatchEvent() {
      return true;
    },
    defaultView: null,
    activeElement: null,
  };

  doc.defaultView = globalThis;
  doc.documentElement = createFakeElement("html", doc);
  doc.head = createFakeElement("head", doc);
  doc.body = createFakeElement("body", doc);
  doc.documentElement.appendChild(doc.head);
  doc.documentElement.appendChild(doc.body);

  globalThis.document = doc;
  globalThis.window = globalThis as any;
  (globalThis as any).HTMLCanvasElement = class HTMLCanvasElement {};
  (globalThis as any).HTMLDivElement = class HTMLDivElement {};
  (globalThis as any).HTMLIFrameElement = class HTMLIFrameElement {};
  (globalThis as any).Element = class Element {};
  (globalThis as any).Node = class Node {};
}

// Track renderer mock lifecycle
let createRendererCallCount = 0;
let disposeCallCount = 0;
let renderCallCount = 0;
let mockFrameWidth = 1920;
let mockFrameHeight = 1080;

mock.module("@/lib/renderer", () => {
  return {
    createRenderer: (_canvas: HTMLCanvasElement) => {
      createRendererCallCount++;
      return {
        backend: "webgl2" as const,
        render: (_buf: ArrayBuffer) => {
          renderCallCount++;
        },
        dispose: () => {
          disposeCallCount++;
        },
      };
    },
    parseFrame: (buf: ArrayBuffer) => {
      if (!buf || buf.byteLength < 16) return null;
      return {
        width: mockFrameWidth,
        height: mockFrameHeight,
        y: new Uint8Array(0),
        uvStride: mockFrameWidth,
        uv: new Uint8Array(0),
        cursor: { x: 0.5, y: 0.5, visible: true },
      };
    },
  };
});

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { SessionCanvas, updateRemoteCursor } from "./SessionCanvas";

async function waitFor(
  predicate: () => boolean | Promise<boolean>,
  options?: { timeoutMs?: number; message?: string }
): Promise<void> {
  const timeoutMs = options?.timeoutMs ?? 2000;
  const startTime = Date.now();
  while (true) {
    if (await predicate()) {
      return;
    }
    if (Date.now() - startTime >= timeoutMs) {
      throw new Error(options?.message ?? `waitFor condition timed out after ${timeoutMs}ms`);
    }
    await new Promise<void>((resolve) => {
      if (typeof setImmediate === "function") {
        setImmediate(resolve);
      } else {
        queueMicrotask(resolve);
      }
    });
  }
}

function findElement(node: any, predicate: (el: any) => boolean): any {
  if (!node) return null;
  if (node.nodeType === 1 && predicate(node)) return node;
  for (const child of node.childNodes || []) {
    const found = findElement(child, predicate);
    if (found) return found;
  }
  return null;
}

describe("SessionCanvas", () => {
  beforeEach(() => {
    createRendererCallCount = 0;
    disposeCallCount = 0;
    renderCallCount = 0;
    mockFrameWidth = 1920;
    mockFrameHeight = 1080;
  });

  it("drives the drawing buffer from the frame dimensions, not hardcoded JSX attributes", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    mockFrameWidth = 2560;
    mockFrameHeight = 1440;

    let pollCount = 0;
    const pollFrame = async () => {
      pollCount++;
      return new ArrayBuffer(20);
    };

    await act(async () => {
      root.render(
        React.createElement(SessionCanvas, {
          active: true,
          pollFrame,
        })
      );
    });

    const canvas = findElement(container, (el) => el.getAttribute("id") === "video-canvas");
    expect(canvas).not.toBeNull();
    // No hardcoded drawing-buffer attributes may be emitted by the JSX.
    expect(canvas.getAttribute("width")).toBeNull();
    expect(canvas.getAttribute("height")).toBeNull();

    await waitFor(() => canvas.width === 2560 && canvas.height === 1440);
    expect(canvas.width).toBe(2560);
    expect(canvas.height).toBe(1440);

    // A re-render must not reset the drawing buffer back to a fixed size.
    await act(async () => {
      root.render(
        React.createElement(SessionCanvas, {
          active: true,
          pollFrame,
          onCursor: () => {},
        })
      );
    });
    expect(canvas.width).toBe(2560);
    expect(canvas.height).toBe(1440);

    // A stream size change is followed.
    mockFrameWidth = 1280;
    mockFrameHeight = 800;
    await waitFor(() => canvas.width === 1280 && canvas.height === 800);
    expect(canvas.width).toBe(1280);
    expect(canvas.height).toBe(800);

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("THE CRITICAL INVARIANT: creates renderer exactly once across multiple re-renders and disposes on unmount", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);

    const initialPollFrame = async () => null;

    // 1. Initial render / mount
    act(() => {
      root.render(
        React.createElement(SessionCanvas, {
          active: true,
          pollFrame: initialPollFrame,
        })
      );
    });

    expect(createRendererCallCount).toBe(1);
    expect(disposeCallCount).toBe(0);

    // 2. Re-render with active = false (prop change)
    act(() => {
      root.render(
        React.createElement(SessionCanvas, {
          active: false,
          pollFrame: initialPollFrame,
        })
      );
    });

    expect(createRendererCallCount).toBe(1);
    expect(disposeCallCount).toBe(0);

    // 3. Re-render with active = true and new pollFrame reference
    const secondPollFrame = async () => null;
    act(() => {
      root.render(
        React.createElement(SessionCanvas, {
          active: true,
          pollFrame: secondPollFrame,
        })
      );
    });

    expect(createRendererCallCount).toBe(1);
    expect(disposeCallCount).toBe(0);

    // 4. Re-render with onCursor callback prop added
    const cursorFn = () => {};
    act(() => {
      root.render(
        React.createElement(SessionCanvas, {
          active: true,
          pollFrame: secondPollFrame,
          onCursor: cursorFn,
        })
      );
    });

    expect(createRendererCallCount).toBe(1);
    expect(disposeCallCount).toBe(0);

    // 5. Re-render with onError callback prop added
    const errorFn = () => {};
    act(() => {
      root.render(
        React.createElement(SessionCanvas, {
          active: true,
          pollFrame: secondPollFrame,
          onCursor: cursorFn,
          onError: errorFn,
        })
      );
    });

    expect(createRendererCallCount).toBe(1);
    expect(disposeCallCount).toBe(0);

    // 6. Unmount component
    act(() => {
      root.unmount();
    });

    // After unmount: createRenderer still called only 1 time total, dispose called exactly once
    expect(createRendererCallCount).toBe(1);
    expect(disposeCallCount).toBe(1);
  });

  it("drives frames through pollFrame when active", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);

    let pollCount = 0;
    const pollFrame = async () => {
      pollCount++;
      return new ArrayBuffer(20);
    };

    act(() => {
      root.render(
        React.createElement(SessionCanvas, {
          active: true,
          pollFrame,
        })
      );
    });

    // Wait for at least one frame poll to execute
    await waitFor(() => pollCount > 0 && renderCallCount > 0);

    expect(pollCount).toBeGreaterThan(0);
    expect(renderCallCount).toBeGreaterThan(0);

    act(() => {
      root.unmount();
    });
  });

  it("cancels rAF and stops polling when active turns false", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);

    let pollCount = 0;
    const pollFrame = async () => {
      pollCount++;
      return null;
    };

    act(() => {
      root.render(
        React.createElement(SessionCanvas, {
          active: true,
          pollFrame,
        })
      );
    });

    await waitFor(() => pollCount > 0);
    const countWhenActive = pollCount;
    expect(countWhenActive).toBeGreaterThan(0);

    // Deactivate
    act(() => {
      root.render(
        React.createElement(SessionCanvas, {
          active: false,
          pollFrame,
        })
      );
    });

    const countAfterInactive = pollCount;

    // After inactive, poll count should not continue increasing
    await new Promise<void>((resolve) => {
      if (typeof setImmediate === "function") {
        setImmediate(resolve);
      } else {
        queueMicrotask(resolve);
      }
    });
    expect(pollCount).toBeLessThanOrEqual(countAfterInactive + 1);

    act(() => {
      root.unmount();
    });
  });

  describe("updateRemoteCursor", () => {
    it("hides cursor when visible is false or coordinates are negative", () => {
      const cursorEl: any = { style: { display: "block", transform: "" } };
      const canvas: any = {
        width: 1920,
        height: 1080,
        getBoundingClientRect: () => ({ left: 0, top: 0, width: 1920, height: 1080 }),
      };

      updateRemoteCursor(canvas, cursorEl, 0.5, 0.5, false);
      expect(cursorEl.style.display).toBe("none");

      updateRemoteCursor(canvas, cursorEl, -1, 0.5, true);
      expect(cursorEl.style.display).toBe("none");

      updateRemoteCursor(canvas, cursorEl, 0.5, -1, true);
      expect(cursorEl.style.display).toBe("none");
    });

    it("calculates projection transform matching legacy letterbox math", () => {
      const cursorEl: any = { style: { display: "none", transform: "" } };
      // 16:9 canvas matching 16:9 rect at (100, 50)
      const canvas: any = {
        width: 1920,
        height: 1080,
        getBoundingClientRect: () => ({ left: 100, top: 50, width: 1920, height: 1080 }),
      };

      updateRemoteCursor(canvas, cursorEl, 0.5, 0.5, true);
      expect(cursorEl.style.display).toBe("block");
      // displayW = 1920, displayH = 1080, offsetX = 0, offsetY = 0
      // px = 100 + 0 + 0.5 * 1920 = 1060
      // py = 50 + 0 + 0.5 * 1080 = 590
      expect(cursorEl.style.transform).toBe("translate(1060px, 590px)");
    });

    it("calculates pillarbox projection when canvas aspect is wider than video aspect", () => {
      const cursorEl: any = { style: { display: "none", transform: "" } };
      // Video aspect = 16:9 = 1.777...
      // Canvas aspect = 2000 / 1000 = 2.0 (wider than video)
      const canvas: any = {
        width: 1600,
        height: 900,
        getBoundingClientRect: () => ({ left: 0, top: 0, width: 2000, height: 1000 }),
      };

      updateRemoteCursor(canvas, cursorEl, 0.5, 0.5, true);
      // videoAspect = 16/9
      // displayH = 1000
      // displayW = 1000 * (16 / 9) = 1777.777...
      // offsetX = (2000 - 1777.777...) / 2 = 111.111...
      // px = 0 + 111.111 + 0.5 * 1777.777 = 1000
      // py = 0 + 0 + 0.5 * 1000 = 500
      expect(cursorEl.style.display).toBe("block");
      expect(cursorEl.style.transform).toBe("translate(1000px, 500px)");
    });
  });
});
