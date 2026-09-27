import { describe, it, expect } from "bun:test";

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

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { HostGrid, type Host } from "./HostGrid";

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
  let text = "";
  if (node.nodeType === 3 && typeof node.textContent === "string") {
    text += node.textContent;
  }
  if (node.childNodes) {
    for (const child of node.childNodes) {
      text += collectText(child);
    }
  }
  return text;
}

describe("HostGrid", () => {
  const mockHosts: Host[] = [
    {
      id: "host-1",
      name: "Desktop-Alpha",
      ip: "192.168.1.10",
      os: "Windows",
      online: true,
      paired: true,
    },
    {
      id: "host-2",
      name: "Server-Beta",
      ip: "192.168.1.20",
      os: "Linux",
      online: false,
      paired: false,
    },
  ];

  it("proves grid rendering, controls, attributes, and interactions", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);

    const connectCalls: any[] = [];

    const onConnect = (host: { id: string; ip: string; name: string }) => {
      connectCalls.push(host);
    };

    act(() => {
      root.render(
        React.createElement(HostGrid, {
          hosts: mockHosts,
          busy: false,
          onConnect,
        })
      );
    });

    // 1. the grid container carries role="list" and exactly 2 elements carry role="listitem"
    const listContainers = findElements(
      container,
      (el) => el.getAttribute("role") === "list"
    );
    expect(listContainers.length).toBe(1);
    const gridContainer = listContainers[0];
    expect(gridContainer.getAttribute("role")).toBe("list");

    const listItems = findElements(
      container,
      (el) => el.getAttribute("role") === "listitem"
    );
    expect(listItems.length).toBe(2);

    // 2. both host names and both ip strings appear in the rendered output
    const allText = collectText(container);
    expect(allText).toContain("Desktop-Alpha");
    expect(allText).toContain("192.168.1.10");
    expect(allText).toContain("Server-Beta");
    expect(allText).toContain("192.168.1.20");

    // 4. the offline host's Connect control is disabled and the online host's Connect control is not
    const connectControls = findElements(
      container,
      (el) => el.getAttribute("data-action") === "connect"
    );
    expect(connectControls.length).toBe(2);

    const onlineConnect = connectControls[0];
    const offlineConnect = connectControls[1];

    const isOnlineDisabled =
      onlineConnect.disabled === true ||
      onlineConnect.getAttribute("disabled") !== null;
    const isOfflineDisabled =
      offlineConnect.disabled === true ||
      offlineConnect.getAttribute("disabled") !== null;

    expect(isOnlineDisabled).toBe(false);
    expect(isOfflineDisabled).toBe(true);

    // 5. invoking the online host's Connect onClick reports that host
    expect(typeof onlineConnect.onClick).toBe("function");
    onlineConnect.onClick();
    expect(connectCalls).toEqual([{ id: "host-1", ip: "192.168.1.10", name: "Desktop-Alpha" }]);

    act(() => {
      root.unmount();
    });
  });

  it("5. when statusMessage is set to a non-empty string, the grid is NOT rendered and the message text IS", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);

    act(() => {
      root.render(
        React.createElement(HostGrid, {
          hosts: mockHosts,
          busy: false,
          onConnect: () => {},
          statusMessage: "No hosts discovered on network",
        })
      );
    });

    // Grid (role="list") should not be rendered
    const listContainers = findElements(
      container,
      (el) => el.getAttribute("role") === "list"
    );
    expect(listContainers.length).toBe(0);

    const listItems = findElements(
      container,
      (el) => el.getAttribute("role") === "listitem"
    );
    expect(listItems.length).toBe(0);

    // Message text IS rendered
    const allText = collectText(container);
    expect(allText).toContain("No hosts discovered on network");

    act(() => {
      root.unmount();
    });
  });
});
