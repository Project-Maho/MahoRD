import { describe, it, expect, beforeEach, mock } from "bun:test";

let hostsFixture: any[] = [];
let listHostsCalls = 0;
let failNextListHosts = 0;
let forgetCalls: string[] = [];
let forgetMode: "resolve" | "reject" = "resolve";
let forgetGate: Promise<void> | null = null;
let hideCalls: string[] = [];
let connectCode: string | null = null;

async function runForget(id: string): Promise<void> {
  forgetCalls.push(id);
  if (forgetGate) await forgetGate;
  if (forgetMode === "reject") throw new Error("pairing store is locked");
  hostsFixture = hostsFixture.map((host) =>
    host.id === id ? { ...host, paired: false } : host
  );
}

mock.module("@/lib/ipc", () => ({
  isNativeAvailable: () => true,
  invokeCommand: async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "list_hosts") {
      listHostsCalls += 1;
      if (failNextListHosts > 0) {
        failNextListHosts -= 1;
        throw new Error("discovery service unavailable");
      }
      return hostsFixture.map((host) => ({ ...host }));
    }
    if (cmd === "forget_pairing") {
      await runForget(String(args?.id ?? ""));
      return null;
    }
    return null;
  },
  forgetPairing: async (id: string) => {
    await runForget(id);
  },
  hideHost: async (name: string) => {
    hideCalls.push(name);
    hostsFixture = hostsFixture.filter(host => host.name !== name);
  },
  agentReleaseAll: async () => {},
}));

if (typeof globalThis.document === "undefined") {
  function createFakeElement(tag: string, ownerDoc: any): any {
    const el: any = {
      tagName: (tag || "div").toUpperCase(),
      nodeType: 1,
      childNodes: [] as any[],
      children: [] as any[],
      style: {} as Record<string, string>,
      attributes: {} as Record<string, string>,
      disabled: false,
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
        if (typeof this.onClick === "function") {
          this.onClick({
            currentTarget: this,
            target: this,
            preventDefault() {},
            stopPropagation() {},
          });
        }
      },
      setAttribute(k: string, v: string) {
        this.attributes[k] = String(v);
        if (k === "disabled") {
          this.disabled = true;
        }
      },
      getAttribute(k: string) {
        return this.attributes[k] ?? null;
      },
      removeAttribute(k: string) {
        delete this.attributes[k];
        if (k === "disabled") {
          this.disabled = false;
        }
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
        if (n.nodeType === 1) this.children.push(n);
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
      get textContent(): string {
        return this.childNodes.map((c: any) => c.textContent ?? "").join("");
      },
      set textContent(v: string) {
        this.childNodes = [ownerDoc.createTextNode(v)];
        this.children = [];
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
      return {
        nodeType: 3,
        textContent: String(text),
        nodeValue: String(text),
        ownerDocument: doc,
        parentNode: null,
      };
    },
    createComment(data: string) {
      return { nodeType: 8, data, ownerDocument: doc, parentNode: null };
    },
    createDocumentFragment() {
      const f = createFakeElement("#document-fragment", doc);
      f.nodeType = 11;
      return f;
    },
    getElementById() {
      return null;
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
  (globalThis as any).HTMLButtonElement = class HTMLButtonElement {};
  (globalThis as any).HTMLIFrameElement = class HTMLIFrameElement {};
  (globalThis as any).Element = class Element {};
  (globalThis as any).Node = class Node {};
}

if (typeof document.getElementById !== "function") {
  document.getElementById = () => null;
}

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { ComputersPage } from "./ComputersPage";
import { createConnection } from "@/lib/connection";

function findElements(node: any, predicate: (el: any) => boolean): any[] {
  const results: any[] = [];
  function walk(current: any) {
    if (!current) return;
    if (predicate(current)) {
      results.push(current);
    }
    if (current.children) {
      for (const child of current.children) {
        walk(child);
      }
    }
  }
  walk(node);
  return results;
}

function collectText(node: any): string {
  if (!node) return "";
  if (node.nodeType === 3) {
    // React updates text children through nodeValue; the fake text node keeps
    // its creation-time textContent as a separate plain property.
    const value = typeof node.nodeValue === "string" ? node.nodeValue : node.textContent;
    return typeof value === "string" ? value : "";
  }
  let text = "";
  if (node.childNodes) {
    for (const child of node.childNodes) {
      text += collectText(child);
    }
  }
  return text;
}

const isDisabled = (el: any) =>
  el.disabled === true || el.getAttribute("disabled") !== null;

async function flush() {
  await act(async () => {
    await Promise.resolve();
  });
}

const connectCalls: any[] = [];
const mockConnection = {
  snapshot: () => ({ busy: false, error: null, errorCode: connectCode, fieldErrors: {} }),
  subscribe: () => () => {},
  connect: async (args: any) => {
    connectCalls.push(args);
    return true;
  },
} as any;

async function renderPage() {
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => {
    root.render(React.createElement(ComputersPage, { connection: mockConnection }));
  });
  await flush();
  return { container, root };
}

describe("ComputersPage paired-machine removal", () => {
  beforeEach(() => {
    hostsFixture = [
      {
        id: "host-1",
        name: "Studio-Desk",
        ip: "10.0.0.5",
        os: "macOS",
        online: true,
        paired: true,
        tcp_port: 19730,
        udp_port: 19731,
      },
      {
        id: "host-2",
        name: "Lab-Box",
        ip: "10.0.0.7",
        os: "Linux",
        online: true,
        paired: false,
        tcp_port: 19730,
        udp_port: 19731,
      },
    ];
    listHostsCalls = 0;
    failNextListHosts = 0;
    forgetCalls = [];
    forgetMode = "resolve";
    forgetGate = null;
    connectCalls.length = 0;
    hideCalls = [];
    connectCode = null;
  });

  it("hides an unpaired host without offering credential deletion", async () => {
    const { container, root } = await renderPage();
    await act(async () => {
      findElements(container, el => el.getAttribute("data-action") === "remove-host")[1].onClick();
    });
    expect(findElements(container, el => el.getAttribute("data-action") === "confirm-forget-paired")).toHaveLength(0);
    await act(async () => {
      await findElements(container, el => el.getAttribute("data-action") === "confirm-remove-paired")[0].onClick();
    });
    expect(hideCalls).toEqual(["Lab-Box"]);
    expect(forgetCalls).toEqual([]);
    act(() => root.unmount());
  });

  it("tries PIN-less connect and falls back only for pairing-required", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);
    const connection = createConnection({ nativeAvailable: true, releaseInputs: async () => {},
      invoke: async (command, args) => {
        if (command === "connect") {
          connectCalls.push(args);
          throw { code: connectCode, message: "Connection failed", stage: "pairing", retryable: false };
        }
      },
    });
    await act(async () => root.render(React.createElement(ComputersPage, { connection })));
    for (const code of ["network-unreachable", "pairing-required"]) {
      connectCode = code;
      await act(async () => {
        await findElements(container, el => el.getAttribute("data-action") === "connect")[1].onClick();
      });
      expect(connectCalls.at(-1)).toMatchObject({ host: "10.0.0.7", pin: null, tcpPort: 19730, udpPort: 19731 });
      expect(connectCalls.at(-1).pairingId ?? null).toBeNull();
      const ip = findElements(container, el => el.getAttribute("id") === "direct-ip")[0];
      expect(ip.value).toBe(code === "pairing-required" ? "10.0.0.7" : "");
    }
    act(() => root.unmount());
  });

  it("polls visible pages silently and clears its interval on unmount", async () => {
    const originalSet = globalThis.setInterval;
    const originalClear = globalThis.clearInterval;
    const originalAdd = document.addEventListener;
    const originalRemove = document.removeEventListener;
    const originalVisibility = Object.getOwnPropertyDescriptor(document, "visibilityState");
    let tick!: () => void;
    let visibility!: () => void;
    let cleared = false;
    globalThis.setInterval = ((fn: () => void, ms: number) => { expect(ms).toBe(4000); tick = fn; return 123; }) as any;
    globalThis.clearInterval = ((id: number) => { expect(id).toBe(123); cleared = true; }) as any;
    document.addEventListener = ((name: string, fn: () => void) => { if (name === "visibilitychange") visibility = fn; }) as any;
    document.removeEventListener = ((name: string, fn: () => void) => { if (name === "visibilitychange") expect(fn).toBe(visibility); }) as any;
    try {
      const { container, root } = await renderPage();
      const initial = listHostsCalls;
      Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
      await act(async () => tick());
      expect(listHostsCalls).toBe(initial);
      Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
      await act(async () => { visibility(); tick(); });
      expect(listHostsCalls).toBe(initial + 2);
      expect(findElements(container, el => el.getAttribute("id") === "btn-refresh")[0].getAttribute("aria-busy")).toBe("false");
      act(() => root.unmount());
      expect(cleared).toBe(true);
    } finally {
      globalThis.setInterval = originalSet;
      globalThis.clearInterval = originalClear;
      document.addEventListener = originalAdd;
      document.removeEventListener = originalRemove;
      if (originalVisibility) Object.defineProperty(document, "visibilityState", originalVisibility);
      else delete (document as any).visibilityState;
    }
  });

  it("confirms with machine name and address before deleting anything", async () => {
    const { container, root } = await renderPage();

    const removeControls = findElements(
      container,
      (el) => el.getAttribute("data-action") === "remove-host"
    );
    expect(removeControls.length).toBe(2);

    await act(async () => {
      removeControls[0].onClick();
    });

    const dialog = findElements(
      container,
      (el) => el.getAttribute("data-testid") === "remove-pairing-confirm"
    )[0];
    expect(dialog).toBeDefined();
    expect(dialog.getAttribute("role")).toBe("alertdialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");

    const describedBy = dialog.getAttribute("aria-describedby");
    expect(describedBy).toBe("remove-pairing-body");
    const body = findElements(container, (el) => el.getAttribute("id") === describedBy)[0];
    expect(body).toBeDefined();
    expect(collectText(body).length).toBeGreaterThan(0);

    const dialogText = collectText(dialog);
    expect(dialogText).toContain("Studio-Desk");
    expect(dialogText).toContain("10.0.0.5");
    expect(forgetCalls).toEqual([]);

    await act(async () => {
      root.unmount();
    });
  });

  it("blocks duplicate confirms while a removal is pending, then refreshes", async () => {
    const { container, root } = await renderPage();
    const callsAfterMount = listHostsCalls;

    await act(async () => {
      findElements(container, (el) => el.getAttribute("data-action") === "remove-host")[0].onClick();
    });

    let release!: () => void;
    forgetGate = new Promise<void>((resolve) => {
      release = resolve;
    });

    const confirm = () =>
      findElements(container, (el) => el.getAttribute("data-action") === "confirm-forget-paired")[0];

    await act(async () => {
      confirm().onClick();
    });
    await flush();

    expect(forgetCalls).toEqual(["host-1"]);
    expect(isDisabled(confirm())).toBe(true);
    expect(collectText(confirm())).toBe("Removing");

    await act(async () => {
      confirm().onClick();
    });
    expect(forgetCalls).toEqual(["host-1"]);

    forgetGate = null;
    await act(async () => {
      release();
    });
    await flush();

    expect(
      findElements(container, (el) => el.getAttribute("data-testid") === "remove-pairing-confirm").length
    ).toBe(0);
    expect(listHostsCalls).toBe(callsAfterMount + 1);
    expect(collectText(container)).toContain("PIN may be needed");
    expect(
      findElements(container, (el) => el.getAttribute("data-action") === "remove-host").length
    ).toBe(1);

    await act(async () => {
      root.unmount();
    });
  });

  it("keeps the dialog open with a visible error and retries after failure", async () => {
    const { container, root } = await renderPage();

    await act(async () => {
      findElements(container, (el) => el.getAttribute("data-action") === "remove-host")[0].onClick();
    });

    forgetMode = "reject";

    const confirm = () =>
      findElements(container, (el) => el.getAttribute("data-action") === "confirm-forget-paired")[0];

    await act(async () => {
      confirm().onClick();
    });
    await flush();

    const errorRegion = findElements(
      container,
      (el) => el.getAttribute("id") === "remove-pairing-error"
    )[0];
    expect(errorRegion).toBeDefined();
    expect(errorRegion.getAttribute("role")).toBe("alert");
    expect(collectText(errorRegion)).toContain("pairing store is locked");
    expect(
      findElements(container, (el) => el.getAttribute("data-testid") === "remove-pairing-confirm").length
    ).toBe(1);
    expect(isDisabled(confirm())).toBe(false);
    expect(forgetCalls).toEqual(["host-1"]);

    forgetMode = "resolve";
    await act(async () => {
      confirm().onClick();
    });
    await flush();

    expect(forgetCalls).toEqual(["host-1", "host-1"]);
    expect(
      findElements(container, (el) => el.getAttribute("data-testid") === "remove-pairing-confirm").length
    ).toBe(0);
    expect(collectText(container)).toContain("PIN may be needed");

    await act(async () => {
      root.unmount();
    });
  });

  it("cannot reuse a stale paired card when the post-removal refresh fails", async () => {
    const { container, root } = await renderPage();

    failNextListHosts = 1;

    await act(async () => {
      findElements(container, (el) => el.getAttribute("data-action") === "remove-host")[0].onClick();
    });
    await act(async () => {
      findElements(container, (el) => el.getAttribute("data-action") === "confirm-forget-paired")[0].onClick();
    });
    await flush();

    expect(forgetCalls).toEqual(["host-1"]);
    expect(collectText(container)).toContain("Studio-Desk");
    expect(collectText(container)).toContain("PIN may be needed");

    const connect = findElements(
      container,
      (el) => el.getAttribute("data-action") === "connect"
    )[0];
    await act(async () => {
      connect.onClick();
    });
    await flush();

    expect(connectCalls[0]).toMatchObject({ pin: null, pairingId: null });

    await act(async () => {
      root.unmount();
    });
  });
});
