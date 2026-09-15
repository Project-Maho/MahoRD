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

let sentInputs: any[] = [];

mock.module("@/lib/ipc", () => {
  return {
    sendInput: async (event: any) => {
      sentInputs.push(event);
    },
    pollFrameRaw: async () => null,
    agentReleaseAll: async () => 0,
    disconnect: async () => {},
    stats: async () => null,
    isNativeAvailable: () => true,
    invokeCommand: async () => {},
  };
});

// Setup minimal DOM if not present
if (typeof globalThis.document === "undefined") {
  function createFakeElement(tag: string, ownerDoc: any): any {
    const el: any = {
      tagName: (tag || "div").toUpperCase(),
      id: "",
      nodeType: 1,
      childNodes: [] as any[],
      children: [] as any[],
      style: {} as Record<string, string>,
      attributes: {} as Record<string, string>,
      width: 1920,
      height: 1080,
      clientWidth: 1920,
      clientHeight: 1080,
      listeners: {} as Record<string, Function[]>,
      setAttribute(k: string, v: string) {
        this.attributes[k] = String(v);
        if (k === "id") this.id = String(v);
      },
      getAttribute(k: string) {
        if (k === "id") return this.id || this.attributes[k] || null;
        return this.attributes[k] ?? null;
      },
      removeAttribute(k: string) {
        delete this.attributes[k];
      },
      closest(selector: string) {
        if (selector.startsWith("#")) {
          const targetId = selector.slice(1);
          let curr: any = this;
          while (curr) {
            if (curr.id === targetId || curr.getAttribute?.("id") === targetId) return curr;
            curr = curr.parentNode;
          }
        }
        return null;
      },
      addEventListener(type: string, fn: Function) {
        (this.listeners[type] ||= []).push(fn);
      },
      removeEventListener(type: string, fn: Function) {
        if (this.listeners[type]) {
          this.listeners[type] = this.listeners[type].filter((l: any) => l !== fn);
        }
      },
      dispatchEvent(e: any) {
        try {
          if (!e.target) Object.defineProperty(e, "target", { value: this, configurable: true });
          if (!e.currentTarget) Object.defineProperty(e, "currentTarget", { value: this, configurable: true });
        } catch (_) {}
        const list = [...(this.listeners[e.type] || [])];
        for (const fn of list) fn(e);
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
      ownerDocument: ownerDoc,
      parentNode: null,
      getBoundingClientRect() {
        return { left: 0, top: 0, width: 1920, height: 1080, right: 1920, bottom: 1080 };
      },
    };
    return el;
  }

  const doc: any = {
    nodeType: 9,
    ownerDocument: null,
    hidden: false,
    pointerLockElement: null,
    listeners: {} as Record<string, Function[]>,
    createElement(tag: string) {
      return createFakeElement(tag, doc);
    },
    createElementNS(_ns: string, tag: string) {
      return createFakeElement(tag, doc);
    },
    createTextNode(text: string) {
      return { nodeType: 3, textContent: text, ownerDocument: doc, parentNode: null };
    },
    addEventListener(type: string, fn: Function) {
      (this.listeners[type] ||= []).push(fn);
    },
    removeEventListener(type: string, fn: Function) {
      if (this.listeners[type]) {
        this.listeners[type] = this.listeners[type].filter((l: any) => l !== fn);
      }
    },
    dispatchEvent(e: any) {
      const list = [...(this.listeners[e.type] || [])];
      for (const fn of list) fn(e);
      return true;
    },
    exitPointerLock() {
      doc.pointerLockElement = null;
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
}

// Enhance document and element methods if already partially defined
if (!globalThis.document.exitPointerLock) {
  globalThis.document.exitPointerLock = () => {
    (globalThis.document as any).pointerLockElement = null;
  };
}

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
import { useRemoteInput, type UseRemoteInputOptions, type RemoteInputHandle } from "./useRemoteInput";

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

function HookTestRig(props: UseRemoteInputOptions & { onMount?: (h: RemoteInputHandle) => void }) {
  const handle = useRemoteInput(props);
  React.useEffect(() => {
    props.onMount?.(handle);
  }, [handle]);
  return null;
}

function createFakeDomTarget(id: string, tag = "div") {
  const listeners: Record<string, Function[]> = {};
  const el: any = {
    id,
    tagName: tag.toUpperCase(),
    width: 1920,
    height: 1080,
    clientWidth: 1920,
    clientHeight: 1080,
    focusCalls: 0,
    focus() {
      this.focusCalls++;
    },
    getBoundingClientRect() {
      return { left: 0, top: 0, width: 1920, height: 1080, right: 1920, bottom: 1080 };
    },
    closest(sel: string) {
      if (sel === `#${this.id}`) return this;
      return null;
    },
    addEventListener(type: string, fn: Function) {
      (listeners[type] ||= []).push(fn);
    },
    removeEventListener(type: string, fn: Function) {
      if (listeners[type]) {
        listeners[type] = listeners[type].filter((l) => l !== fn);
      }
    },
    dispatchEvent(e: any) {
      try {
        if (!e.target) Object.defineProperty(e, "target", { value: this, configurable: true });
        if (!e.currentTarget) Object.defineProperty(e, "currentTarget", { value: this, configurable: true });
      } catch (_) {}
      const list = [...(listeners[e.type] || [])];
      for (const fn of list) fn(e);
      return true;
    },
  };
  return el;
}

describe("useRemoteInput", () => {
  beforeEach(() => {
    sentInputs = [];
  });

  it("keydown with repeat true is forwarded so held keys repeat on the host", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
        })
      );
    });

    // Dispatch keydown with repeat: true targeting viewport
    let repeatPrevented = false;
    const repeatEvent = new Event("keydown") as any;
    repeatEvent.key = "Enter";
    repeatEvent.keyCode = 13;
    repeatEvent.repeat = true;
    repeatEvent.preventDefault = () => {
      repeatPrevented = true;
    };
    Object.defineProperty(repeatEvent, "target", {
      value: { tagName: "DIV", id: "viewport" },
    });

    window.dispatchEvent(repeatEvent);
    expect(sentInputs.filter((i) => i.event_type === "KeyDown").length).toBe(1);
    // Forwarded keys are consumed locally: Tab must not move focus out of the session.
    expect(repeatPrevented).toBe(true);

    // Dispatch keydown with repeat: false targeting viewport
    const normalEvent = new Event("keydown") as any;
    normalEvent.key = "Enter";
    normalEvent.keyCode = 13;
    normalEvent.repeat = false;
    Object.defineProperty(normalEvent, "target", {
      value: { tagName: "DIV", id: "viewport" },
    });

    window.dispatchEvent(normalEvent);
    const downInputs = sentInputs.filter((i) => i.event_type === "KeyDown");
    expect(downInputs.length).toBe(2);
    expect(downInputs[0].key_code).toBe(13);
    expect(downInputs[1].key_code).toBe(13);

    act(() => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("a drag that leaves the viewport keeps sending move events from the window", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");
    viewport.contains = (node: any) => node === viewport || node === canvas;

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
        })
      );
    });

    const downEv = new Event("mousedown") as any;
    downEv.button = 0;
    downEv.clientX = 100;
    downEv.clientY = 100;
    viewport.dispatchEvent(downEv);
    expect(sentInputs.filter((i) => i.event_type === "LeftMouseDown").length).toBe(1);

    sentInputs = [];
    // Move with the button held, targeting an element OUTSIDE the viewport.
    const outside = createFakeDomTarget("outside");
    const dragEv = new Event("mousemove") as any;
    dragEv.clientX = 4000;
    dragEv.clientY = 100;
    Object.defineProperty(dragEv, "target", { value: outside, configurable: true });
    window.dispatchEvent(dragEv);

    // Motion is coalesced through rAF: flushed by releaseAll on teardown.
    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
    await waitFor(() => sentInputs.some((i) => i.event_type === "LeftMouseDragged"));
    expect(sentInputs.filter((i) => i.event_type === "LeftMouseDragged").length).toBe(1);
  });

  it("hover motion outside the viewport with no button held is not forwarded", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");
    viewport.contains = (node: any) => node === viewport || node === canvas;

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
        })
      );
    });

    const outside = createFakeDomTarget("outside");
    const moveEv = new Event("mousemove") as any;
    moveEv.clientX = 10;
    moveEv.clientY = 10;
    Object.defineProperty(moveEv, "target", { value: outside, configurable: true });
    window.dispatchEvent(moveEv);

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
    expect(sentInputs.filter((i) => i.event_type === "MouseMove").length).toBe(0);
  });

  it("rebinds to a new viewport element and releases held inputs on disconnect", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const firstViewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport: firstViewport,
          canvas,
        })
      );
    });

    // Hold a button on the first viewport.
    const downEv = new Event("mousedown") as any;
    downEv.button = 0;
    downEv.clientX = 10;
    downEv.clientY = 10;
    firstViewport.dispatchEvent(downEv);
    expect(sentInputs.filter((i) => i.event_type === "LeftMouseDown").length).toBe(1);

    // Swap in a different viewport element: the effect must re-run, releasing
    // the held button instead of orphaning it against a stale element.
    sentInputs = [];
    const secondViewport = createFakeDomTarget("viewport");
    await act(async () => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport: secondViewport,
          canvas,
        })
      );
    });
    await waitFor(() => sentInputs.filter((i) => i.event_type === "LeftMouseUp").length === 1);

    // The stale element no longer drives input; the current one does.
    sentInputs = [];
    const staleEv = new Event("mousedown") as any;
    staleEv.button = 0;
    staleEv.clientX = 20;
    staleEv.clientY = 20;
    firstViewport.dispatchEvent(staleEv);
    expect(sentInputs.filter((i) => i.event_type === "LeftMouseDown").length).toBe(0);

    const freshEv = new Event("mousedown") as any;
    freshEv.button = 0;
    freshEv.clientX = 30;
    freshEv.clientY = 30;
    secondViewport.dispatchEvent(freshEv);
    expect(sentInputs.filter((i) => i.event_type === "LeftMouseDown").length).toBe(1);

    // Disconnecting re-runs the effect and releases what is still held.
    sentInputs = [];
    await act(async () => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: false,
          viewport: secondViewport,
          canvas,
        })
      );
    });
    await waitFor(() => sentInputs.filter((i) => i.event_type === "LeftMouseUp").length === 1);

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("the modifier bitmask is shift=1, ctrl=2, alt=4, meta=8 and combines correctly", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
        })
      );
    });

    const combinations = [
      { shiftKey: false, ctrlKey: false, altKey: false, metaKey: false, expected: 0 },
      { shiftKey: true, ctrlKey: false, altKey: false, metaKey: false, expected: 1 },
      { shiftKey: false, ctrlKey: true, altKey: false, metaKey: false, expected: 2 },
      { shiftKey: false, ctrlKey: false, altKey: true, metaKey: false, expected: 4 },
      { shiftKey: false, ctrlKey: false, altKey: false, metaKey: true, expected: 8 },
      { shiftKey: true, ctrlKey: true, altKey: false, metaKey: false, expected: 3 },
      { shiftKey: true, ctrlKey: false, altKey: true, metaKey: false, expected: 5 },
      { shiftKey: false, ctrlKey: true, altKey: true, metaKey: false, expected: 6 },
      { shiftKey: true, ctrlKey: true, altKey: true, metaKey: false, expected: 7 },
      { shiftKey: false, ctrlKey: false, altKey: true, metaKey: true, expected: 12 },
      { shiftKey: true, ctrlKey: false, altKey: true, metaKey: true, expected: 13 },
      { shiftKey: true, ctrlKey: true, altKey: true, metaKey: true, expected: 15 },
    ];

    for (let idx = 0; idx < combinations.length; idx++) {
      sentInputs = [];
      const combo = combinations[idx];
      const ev = new Event("keydown") as any;
      ev.keyCode = 65 + idx;
      ev.repeat = false;
      ev.shiftKey = combo.shiftKey;
      ev.ctrlKey = combo.ctrlKey;
      ev.altKey = combo.altKey;
      ev.metaKey = combo.metaKey;
      Object.defineProperty(ev, "target", {
        value: { tagName: "DIV", id: "viewport" },
      });

      window.dispatchEvent(ev);

      const downs = sentInputs.filter((i) => i.event_type === "KeyDown");
      expect(downs.length).toBe(1);
      expect(downs[0].modifiers).toBe(combo.expected);
    }

    act(() => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("a held button releases exactly once on teardown and is not double-sent", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");
    let handleRef: RemoteInputHandle | null = null;

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
          onMount: (h: RemoteInputHandle) => {
            handleRef = h;
          },
        })
      );
    });

    // Send left mouse down on viewport (button = 0)
    const downEv = new Event("mousedown") as any;
    downEv.button = 0;
    downEv.clientX = 100;
    downEv.clientY = 100;
    viewport.dispatchEvent(downEv);

    // Verify LeftMouseDown was sent
    const downInputs = sentInputs.filter((i) => i.event_type === "LeftMouseDown");
    expect(downInputs.length).toBe(1);

    // Clear sent inputs before unmount
    sentInputs = [];

    // Teardown: unmount the hook
    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
    await waitFor(() => sentInputs.filter((i) => i.event_type === "LeftMouseUp").length === 1);

    // Exactly one LeftMouseUp event should have been emitted during teardown
    const upInputsAfterTeardown = sentInputs.filter((i) => i.event_type === "LeftMouseUp");
    expect(upInputsAfterTeardown.length).toBe(1);

    // Second call to releaseAll (or double-teardown): must not emit any further release events
    await (handleRef as RemoteInputHandle | null)?.releaseAll?.();
    const upInputsAfterSecondRelease = sentInputs.filter((i) => i.event_type === "LeftMouseUp");
    expect(upInputsAfterSecondRelease.length).toBe(1);
  });

  it("releaseAll function returned can be invoked directly", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");
    let handleRef: RemoteInputHandle | null = null;

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
          onMount: (h: RemoteInputHandle) => {
            handleRef = h;
          },
        })
      );
    });

    // Press middle button (button = 1)
    let prevented = false;
    const downEv = new Event("mousedown") as any;
    downEv.button = 1;
    downEv.clientX = 50;
    downEv.clientY = 50;
    downEv.preventDefault = () => {
      prevented = true;
    };
    viewport.dispatchEvent(downEv);

    expect(prevented).toBe(true);
    expect(sentInputs.some((i) => i.event_type === "MiddleMouseDown")).toBe(true);

    sentInputs = [];
    // Invoke releaseAll directly
    await handleRef!();
    expect(sentInputs.filter((i) => i.event_type === "MiddleMouseUp").length).toBe(1);

    // On unmount, already released, so no duplicate
    sentInputs = [];
    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);

    expect(sentInputs.filter((i) => i.event_type === "MiddleMouseUp").length).toBe(0);
  });

  it("releases held inputs on overlay pointerdown, except for #btn-home and #btn-disconnect", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");
    const overlay = createFakeDomTarget("session-overlay");

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
          overlay,
        })
      );
    });

    // Press right button (button = 2)
    const downEv = new Event("mousedown") as any;
    downEv.button = 2;
    downEv.clientX = 200;
    downEv.clientY = 200;
    viewport.dispatchEvent(downEv);
    expect(sentInputs.some((i) => i.event_type === "RightMouseDown")).toBe(true);

    // Overlay pointerdown on home button: should NOT release
    sentInputs = [];
    const homeBtn = { id: "btn-home", closest: (sel: string) => (sel === "#btn-home" ? homeBtn : null) };
    const homePointerEv = new Event("pointerdown") as any;
    Object.defineProperty(homePointerEv, "target", { value: homeBtn, configurable: true });
    overlay.dispatchEvent(homePointerEv);
    expect(sentInputs.filter((i) => i.event_type === "RightMouseUp").length).toBe(0);

    // Overlay pointerdown on disconnect button: should NOT release
    const discBtn = { id: "btn-disconnect", closest: (sel: string) => (sel === "#btn-disconnect" ? discBtn : null) };
    const discPointerEv = new Event("pointerdown") as any;
    Object.defineProperty(discPointerEv, "target", { value: discBtn, configurable: true });
    overlay.dispatchEvent(discPointerEv);
    expect(sentInputs.filter((i) => i.event_type === "RightMouseUp").length).toBe(0);

    // Overlay pointerdown elsewhere: MUST release
    const otherEl = { id: "panel", closest: () => null };
    const normalPointerEv = new Event("pointerdown") as any;
    Object.defineProperty(normalPointerEv, "target", { value: otherEl, configurable: true });
    overlay.dispatchEvent(normalPointerEv);
    await waitFor(() => sentInputs.filter((i) => i.event_type === "RightMouseUp").length === 1);
    expect(sentInputs.filter((i) => i.event_type === "RightMouseUp").length).toBe(1);

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("handles viewport wheel event with preventDefault and clamped coords", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
        })
      );
    });

    let wheelPrevented = false;
    const wheelEv = new Event("wheel") as any;
    wheelEv.clientX = 500;
    wheelEv.clientY = 300;
    wheelEv.deltaX = 12.5;
    wheelEv.deltaY = -24.0;
    wheelEv.preventDefault = () => {
      wheelPrevented = true;
    };

    viewport.dispatchEvent(wheelEv);

    expect(wheelPrevented).toBe(true);
    const wheelInputs = sentInputs.filter((i) => i.event_type === "ScrollWheel");
    expect(wheelInputs.length).toBe(1);
    expect(wheelInputs[0].scroll_dx).toBe(12.5);
    expect(wheelInputs[0].scroll_dy).toBe(-24.0);
    expect(wheelInputs[0].x).toBe(500);
    expect(wheelInputs[0].y).toBe(300);

    act(() => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("maps wheel coords through the letterbox and forwards keyboard modifiers", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");
    // Pillarboxed 16:9 video content centered in a 2000x1000 canvas rect.
    canvas.width = 1600;
    canvas.height = 900;
    canvas.getBoundingClientRect = () => ({
      left: 0,
      top: 0,
      width: 2000,
      height: 1000,
      right: 2000,
      bottom: 1000,
    });

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
        })
      );
    });

    // offsetX = (2000 - 1000 * 16/9) / 2 = 111.111...; a clientX of 200 must
    // map to ~88.9 in video content, NOT 200 against the full canvas rect.
    const wheelEv = new Event("wheel") as any;
    wheelEv.clientX = 200;
    wheelEv.clientY = 450;
    wheelEv.deltaX = 0;
    wheelEv.deltaY = -120;
    wheelEv.shiftKey = true;
    wheelEv.ctrlKey = true;
    wheelEv.altKey = false;
    wheelEv.metaKey = false;
    wheelEv.preventDefault = () => {};

    viewport.dispatchEvent(wheelEv);

    const wheelInputs = sentInputs.filter((i) => i.event_type === "ScrollWheel");
    expect(wheelInputs.length).toBe(1);
    expect(wheelInputs[0].modifiers).toBe(3); // shift=1 | ctrl=2
    expect(wheelInputs[0].x).toBeCloseTo(200 - 111.111, 1);
    expect(wheelInputs[0].y).toBeCloseTo(450, 1);
    expect(wheelInputs[0].view_width).toBeCloseTo(1777.778, 1);
    expect(wheelInputs[0].view_height).toBeCloseTo(1000, 1);

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("mousedown on the viewport focuses the video canvas", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
        })
      );
    });

    const downEv = new Event("mousedown") as any;
    downEv.button = 0;
    downEv.clientX = 10;
    downEv.clientY = 10;
    viewport.dispatchEvent(downEv);

    expect(viewport.focusCalls).toBe(1);
    // The canvas carries tabIndex=0 and is the remote input surface: mousedown
    // must focus it regardless of its id, never skip it.
    expect(canvas.focusCalls).toBe(1);

    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  });

  it("handles viewport contextmenu with preventDefault", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    const viewport = createFakeDomTarget("viewport");
    const canvas = createFakeDomTarget("video-canvas", "canvas");

    act(() => {
      root.render(
        React.createElement(HookTestRig, {
          isConnected: true,
          viewport,
          canvas,
        })
      );
    });

    let cmPrevented = false;
    const cmEv = new Event("contextmenu") as any;
    cmEv.preventDefault = () => {
      cmPrevented = true;
    };

    viewport.dispatchEvent(cmEv);
    expect(cmPrevented).toBe(true);

    act(() => {
      root.unmount();
    });
    document.body.removeChild(container);
  });
});
