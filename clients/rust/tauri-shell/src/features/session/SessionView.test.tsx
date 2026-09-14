import { describe, it, expect, mock, beforeEach } from "bun:test";

// Polyfill classes for React DOM if needed
if (typeof (globalThis as any).HTMLIFrameElement === "undefined") {
  (globalThis as any).HTMLIFrameElement = class HTMLIFrameElement {};
}
if (typeof (globalThis as any).HTMLCanvasElement === "undefined") {
  (globalThis as any).HTMLCanvasElement = class HTMLCanvasElement {};
}
if (typeof (globalThis as any).HTMLDivElement === "undefined") {
  (globalThis as any).HTMLDivElement = class HTMLDivElement {};
}
if (typeof (globalThis as any).Element === "undefined") {
  (globalThis as any).Element = class Element {};
}
if (typeof (globalThis as any).Node === "undefined") {
  (globalThis as any).Node = class Node {};
}

// Mock renderer module before importing React components to avoid WebGL errors in headless test env
mock.module("@/lib/renderer", () => {
  return {
    createRenderer: (_canvas: any) => ({
      backend: "webgl2" as const,
      render: (_buf: ArrayBuffer) => {},
      dispose: () => {},
    }),
    parseFrame: (buf: ArrayBuffer) => {
      if (!buf || buf.byteLength < 16) return null;
      return {
        width: 1920,
        height: 1080,
        y: new Uint8Array(0),
        uv: new Uint8Array(0),
        cursor: { x: 0.5, y: 0.5, visible: true },
      };
    },
  };
});

let agentReleaseAllCalls = 0;
let sentInputs: any[] = [];

mock.module("@/lib/ipc", () => {
  return {
    pollFrameRaw: async () => new ArrayBuffer(20),
    sendInput: async (event: any) => {
      sentInputs.push(event);
    },
    agentReleaseAll: async () => {
      agentReleaseAllCalls++;
      return 0;
    },
    disconnect: async () => {},
    stats: async () => null,
    isNativeAvailable: () => true,
    invokeCommand: async () => {},
  };
});

// Set up minimal DOM environment if document is undefined, or enhance it if already set
if (typeof globalThis.document === "undefined") {
  function createFakeElement(tag: string, ownerDoc: any): any {
    const el: any = {
      tagName: (tag || "div").toUpperCase(),
      nodeType: 1,
      childNodes: [] as any[],
      children: [] as any[],
      style: {} as Record<string, string>,
      attributes: {} as Record<string, string>,
      className: "",
      disabled: false,
      _textContent: undefined as string | undefined,
      get textContent(): string {
        if (this._textContent !== undefined) return this._textContent;
        return (this.childNodes || []).map((c: any) => c.textContent ?? "").join("");
      },
      set textContent(v: string) {
        this._textContent = String(v);
        this.childNodes = [ownerDoc.createTextNode(v)];
      },
      _onClick: undefined as any,
      get onClick() {
        for (const key of Object.keys(this)) {
          if (key.startsWith("__reactProps") && this[key]?.onClick) {
            return this[key].onClick;
          }
        }
        return this._onClick;
      },
      set onClick(fn: any) {
        this._onClick = fn;
      },
      click() {
        for (const key of Object.keys(this)) {
          if (key.startsWith("__reactProps") && typeof this[key]?.onClick === "function") {
            this[key].onClick({
              currentTarget: this,
              target: this,
              preventDefault() {},
              stopPropagation() {},
            });
            return;
          }
        }
        if (typeof this._onClick === "function") {
          this._onClick({
            currentTarget: this,
            target: this,
            preventDefault() {},
            stopPropagation() {},
          });
        }
      },
      setAttribute(k: string, v: string) {
        this.attributes[k] = String(v);
        if (k === "class" || k === "className") {
          this.className = String(v);
        }
      },
      getAttribute(k: string) {
        if (k === "class" || k === "className") {
          return this.className || this.attributes[k] || null;
        }
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
      focus() {},
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

// Enhance document.getElementById and click helper if missing
function findInTree(node: any, predicate: (el: any) => boolean): any {
  if (!node) return null;
  if (predicate(node)) return node;
  const children = node.childNodes || node.children || [];
  for (const child of children) {
    const found = findInTree(child, predicate);
    if (found) return found;
  }
  return null;
}

function findElements(node: any, predicate: (el: any) => boolean): any[] {
  const results: any[] = [];
  function walk(current: any) {
    if (!current) return;
    if (current.nodeType === 1 && typeof current.getAttribute === "function" && predicate(current)) {
      results.push(current);
    }
    const children = current.childNodes || current.children || [];
    for (const child of children) {
      walk(child);
    }
  }
  walk(node);
  return results;
}

function getAllText(node: any): string {
  if (!node) return "";
  let text = "";
  if (node.nodeType === 3) {
    text += node.textContent || "";
  } else if (typeof node.textContent === "string" && (!node.childNodes || node.childNodes.length === 0)) {
    text += node.textContent;
  }
  const children = node.childNodes || node.children || [];
  for (const child of children) {
    text += getAllText(child);
  }
  return text;
}

if (!globalThis.document.getElementById) {
  globalThis.document.getElementById = (id: string) => {
    return findInTree(
      globalThis.document.body || globalThis.document.documentElement,
      (el: any) => el.attributes?.id === id || el.id === id
    );
  };
}

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { SessionView } from "./SessionView";
import App from "@/app/App";
import type { ConnectionInstance, ConnectionSnapshot } from "@/lib/connection";

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

function createMockConnection(phase: ConnectionSnapshot["phase"] = "streaming"): ConnectionInstance {
  const state: ConnectionSnapshot = {
    phase,
    host: { ip: "192.168.1.50", name: "Host-Workstation" },
    fieldErrors: {},
    error: null,
    cleanupError: null,
    stats: {
      connected: true,
      latency_p50_ms: 14.2,
      latency_p99_ms: 28.5,
      frames_decoded: 300,
      frames_received: 310,
      audio_packets_received: 80,
    },
    statsStatus: "ready",
    statsError: null,
    generation: 1,
    busy: true,
  };

  const listeners = new Set<(s: ConnectionSnapshot) => void>();

  return {
    snapshot: () => ({ ...state }),
    subscribe: (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    connect: async () => {
      state.phase = "connecting";
      state.busy = true;
      listeners.forEach((l) => l({ ...state }));
      return true;
    },
    cancel: async () => {},
    disconnect: async () => {
      state.phase = "idle";
      state.busy = false;
      listeners.forEach((l) => l({ ...state }));
    },
    retryCleanup: async () => {},
    refreshStats: async () => {},
    token: () => state.generation,
    isCurrent: () => true,
    markFrameRendered: async () => {},
    reportFrameError: async () => {},
  };
}

function triggerClick(el: any) {
  for (const key of Object.keys(el)) {
    if (key.startsWith("__reactProps") && typeof el[key]?.onClick === "function") {
      el[key].onClick({
        currentTarget: el,
        target: el,
        preventDefault() {},
        stopPropagation() {},
      });
      return;
    }
  }
  if (typeof el.click === "function") {
    el.click();
  }
}

describe("SessionView", () => {
  beforeEach(() => {
    agentReleaseAllCalls = 0;
    sentInputs = [];
  });

  it("renders SessionCanvas and floating session-overlay with pointer-events rules", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const mockConn = createMockConnection("streaming");

    await act(async () => {
      root.render(React.createElement(SessionView, { connection: mockConn }));
    });

    // Viewport and Canvas
    const viewports = findElements(container, (el) => el.getAttribute("id") === "viewport");
    expect(viewports.length).toBe(1);
    expect(viewports[0].getAttribute("tabindex")).toBe("0");

    const canvases = findElements(container, (el) => el.getAttribute("id") === "video-canvas");
    expect(canvases.length).toBe(1);

    // Floating overlay with id "session-overlay"
    const overlays = findElements(container, (el) => el.getAttribute("id") === "session-overlay");
    expect(overlays.length).toBe(1);
    const overlay = overlays[0];
    // Container has pointer-events: none
    const overlayClass = overlay.className || overlay.getAttribute("class") || "";
    expect(overlayClass).toContain("pointer-events-none");

    // Interactive launcher bar has pointer-events: auto
    const bars = findElements(container, (el) => el.getAttribute("id") === "launcher-bar");
    expect(bars.length).toBe(1);
    const barClass = bars[0].className || bars[0].getAttribute("class") || "";
    expect(barClass).toContain("pointer-events-auto");

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("reproduces legacy copy: host name badge, live stats line, and buttons", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const mockConn = createMockConnection("streaming");

    await act(async () => {
      root.render(React.createElement(SessionView, { connection: mockConn }));
    });

    // Host badge contains host name
    const hostBadges = findElements(container, (el) => el.getAttribute("id") === "session-host-name");
    expect(hostBadges.length).toBe(1);
    expect(getAllText(hostBadges[0])).toContain("Host-Workstation");

    // Live stats line contains FPS and latency
    const fpsStats = findElements(container, (el) => el.getAttribute("id") === "session-stat-fps");
    expect(fpsStats.length).toBe(1);
    expect(getAllText(fpsStats[0])).toContain("Rendered FPS");

    const latStats = findElements(container, (el) => el.getAttribute("id") === "session-stat-lat");
    expect(latStats.length).toBe(1);
    expect(getAllText(latStats[0])).toContain("Decode p50");
    expect(getAllText(latStats[0])).toContain("14.2 ms");

    // Home, Expand, and Disconnect buttons with legacy attributes & copy
    const homeBtns = findElements(container, (el) => el.getAttribute("id") === "btn-home");
    expect(homeBtns.length).toBe(1);
    expect(homeBtns[0].getAttribute("title")).toBe("Home — end session and return to main screen");

    const expandBtns = findElements(container, (el) => el.getAttribute("id") === "btn-expand");
    expect(expandBtns.length).toBe(1);
    expect(expandBtns[0].getAttribute("aria-expanded")).toBe("false");

    const dcBtns = findElements(container, (el) => el.getAttribute("id") === "btn-disconnect");
    expect(dcBtns.length).toBe(1);
    expect(getAllText(dcBtns[0])).toContain("Disconnect");

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("toggles launcher panel when Expand button is clicked", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const mockConn = createMockConnection("streaming");

    await act(async () => {
      root.render(React.createElement(SessionView, { connection: mockConn }));
    });

    const expandBtns = findElements(container, (el) => el.getAttribute("id") === "btn-expand");
    expect(expandBtns.length).toBe(1);
    expect(findElements(container, (el) => el.getAttribute("id") === "launcher-panel").length).toBe(0);

    // Click expand
    await act(async () => {
      triggerClick(expandBtns[0]);
    });

    // Launcher panel is now visible
    const panels = findElements(container, (el) => el.getAttribute("id") === "launcher-panel");
    expect(panels.length).toBe(1);
    expect(expandBtns[0].getAttribute("aria-expanded")).toBe("true");

    // Stats grid inside panel
    const states = findElements(container, (el) => el.getAttribute("id") === "session-stat-state");
    expect(states.length).toBe(1);
    expect(getAllText(states[0])).toBe("streaming");

    const p50s = findElements(container, (el) => el.getAttribute("id") === "session-stat-p50");
    expect(p50s.length).toBe(1);
    expect(getAllText(p50s[0])).toBe("14.2 ms");

    const decoded = findElements(container, (el) => el.getAttribute("id") === "session-stat-decoded");
    expect(decoded.length).toBe(1);
    expect(getAllText(decoded[0])).toBe("300");

    // Fullscreen button inside panel
    const fsBtns = findElements(container, (el) => el.getAttribute("id") === "btn-fullscreen");
    expect(fsBtns.length).toBe(1);
    expect(getAllText(fsBtns[0])).toContain("Fullscreen");

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("on teardown releases held inputs via agentReleaseAll", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const mockConn = createMockConnection("streaming");

    await act(async () => {
      root.render(React.createElement(SessionView, { connection: mockConn }));
    });

    expect(agentReleaseAllCalls).toBe(0);

    // Teardown unmount
    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
    await waitFor(() => agentReleaseAllCalls >= 1);

    expect(agentReleaseAllCalls).toBeGreaterThanOrEqual(1);
  });

  describe("App phase routing", () => {
    it("renders the launcher when connection phase is idle", async () => {
      const container = document.createElement("div");
      document.body.appendChild(container);
      const root = createRoot(container);
      const mockConn = createMockConnection("idle");

      await act(async () => {
        root.render(<App connection={mockConn} />);
      });

      // Launcher main-view and sidebar are rendered, session-overlay is not
      const mainView = findElements(container, (el) => el.getAttribute("id") === "main-view");
      expect(mainView.length).toBe(1);

      const overlay = findElements(container, (el) => el.getAttribute("id") === "session-overlay");
      expect(overlay.length).toBe(0);

      await act(async () => {
        root.unmount();
      });
      document.body.removeChild(container);
    });

    it("renders SessionView when connection phase is connecting, waiting-video, or streaming", async () => {
      for (const phase of ["connecting", "waiting-video", "streaming"] as const) {
        const container = document.createElement("div");
        document.body.appendChild(container);
        const root = createRoot(container);
        const mockConn = createMockConnection(phase);

        await act(async () => {
          root.render(<App connection={mockConn} />);
        });

        // SessionView overlay is rendered, launcher main-view is not
        const overlay = findElements(container, (el) => el.getAttribute("id") === "session-overlay");
        expect(overlay.length).toBe(1);

        const mainView = findElements(container, (el) => el.getAttribute("id") === "main-view");
        expect(mainView.length).toBe(0);

        await act(async () => {
          root.unmount();
        });
        document.body.removeChild(container);
      }
    });

    it("passes shared connection to ComputersPage so triggering connect switches App to SessionView", async () => {
      const container = document.createElement("div");
      document.body.appendChild(container);
      const root = createRoot(container);
      const mockConn = createMockConnection("idle");

      await act(async () => {
        root.render(<App connection={mockConn} />);
      });

      expect(findElements(container, (el) => el.getAttribute("id") === "main-view").length).toBe(1);
      expect(findElements(container, (el) => el.getAttribute("id") === "session-overlay").length).toBe(0);

      await act(async () => {
        await mockConn.connect({ host: "100.91.254.71" });
      });

      expect(findElements(container, (el) => el.getAttribute("id") === "main-view").length).toBe(0);
      expect(findElements(container, (el) => el.getAttribute("id") === "session-overlay").length).toBe(1);

      await act(async () => {
        root.unmount();
      });
      document.body.removeChild(container);
    });
  });

  describe("Remote input forwarding", () => {
    it("forwards repeated keyboard events so held keys repeat on the host", async () => {
      const container = document.createElement("div");
      document.body.appendChild(container);
      const root = createRoot(container);
      const mockConn = createMockConnection("streaming");

      await act(async () => {
        root.render(React.createElement(SessionView, { connection: mockConn }));
      });

      const viewports = findElements(container, (el) => el.getAttribute("id") === "viewport");
      expect(viewports.length).toBe(1);

      // Trigger keydown with e.repeat = true
      const repeatEvent = new Event("keydown");
      Object.defineProperty(repeatEvent, "key", { value: "a" });
      Object.defineProperty(repeatEvent, "keyCode", { value: 65 });
      Object.defineProperty(repeatEvent, "repeat", { value: true });
      Object.defineProperty(repeatEvent, "target", {
        value: { tagName: "DIV", id: "viewport" },
      });

      if (typeof window !== "undefined" && typeof (window as any).dispatchEvent === "function") {
        (window as any).dispatchEvent(repeatEvent);
      }

      expect(sentInputs.filter((i) => i.event_type === "KeyDown").length).toBe(1);

      // Non-repeated keydown is forwarded too
      const normalEvent = new Event("keydown");
      Object.defineProperty(normalEvent, "key", { value: "b" });
      Object.defineProperty(normalEvent, "keyCode", { value: 66 });
      Object.defineProperty(normalEvent, "repeat", { value: false });
      Object.defineProperty(normalEvent, "target", {
        value: { tagName: "DIV", id: "viewport" },
      });

      if (typeof window !== "undefined" && typeof (window as any).dispatchEvent === "function") {
        (window as any).dispatchEvent(normalEvent);
      }

      const keyEvents = sentInputs.filter((i) => i.event_type === "KeyDown");
      expect(keyEvents.length).toBe(2);
      expect(keyEvents[0].key_code).toBe(65);
      expect(keyEvents[1].key_code).toBe(66);

      await act(async () => {
        root.unmount();
      });
      document.body.removeChild(container);
    });
  });
});
