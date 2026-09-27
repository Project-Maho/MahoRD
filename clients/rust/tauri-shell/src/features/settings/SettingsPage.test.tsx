import { describe, it, expect, mock, beforeEach } from "bun:test";

let getHostStatusCalls = 0;
let setAutoApproveCalls: boolean[] = [];
let setHostPolicyCalls: any[] = [];
let listPairingsCalls = 0;

let currentHostStatus = {
  running: true,
  ip: "192.168.1.100",
  port: 19730,
  pin: "123456",
  auto_approve: false,
  allow_input: true,
  allow_clipboard: true,
};

mock.module("@/lib/ipc", () => {
  return {
    isNativeAvailable: () => true,
    listPairings: async () => {
      listPairingsCalls++;
      return [];
    },
    forgetPairing: async () => {},
    getHostStatus: async () => {
      getHostStatusCalls++;
      return { ...currentHostStatus };
    },
    startHost: async () => {
      currentHostStatus.running = true;
      return { ...currentHostStatus };
    },
    stopHost: async () => {
      currentHostStatus.running = false;
      return { ...currentHostStatus };
    },
    setAutoApprove: async (enabled: boolean) => {
      setAutoApproveCalls.push(enabled);
      currentHostStatus.auto_approve = enabled;
      return { ...currentHostStatus };
    },
    setHostPolicy: async (args: any) => {
      setHostPolicyCalls.push(args);
      currentHostStatus.allow_input = args.allowInput;
      currentHostStatus.allow_clipboard = args.allowClipboard;
      return { ...currentHostStatus };
    },
    invokeCommand: async () => {},
  };
});

if (typeof (globalThis as any).HTMLFormElement === "undefined") {
  (globalThis as any).HTMLFormElement = class HTMLFormElement {};
}
if (typeof (globalThis as any).HTMLInputElement === "undefined") {
  class FakeHTMLInputElement {
    get value() {
      return (this as any)._val ?? "";
    }
    set value(v: any) {
      (this as any)._val = v;
    }
  }
  (globalThis as any).HTMLInputElement = FakeHTMLInputElement;
}

if (typeof window !== "undefined") {
  if (!(window as any).HTMLFormElement) (window as any).HTMLFormElement = (globalThis as any).HTMLFormElement;
  if (!(window as any).HTMLInputElement) (window as any).HTMLInputElement = (globalThis as any).HTMLInputElement;
}

if (typeof document !== "undefined" && typeof document.createElement === "function") {
  const origCreateElement = document.createElement.bind(document);
  document.createElement = function (tagName: string, options?: any) {
    const el = origCreateElement(tagName, options);
    if (el) {
      if (typeof (el as any).querySelectorAll !== "function") {
        (el as any).querySelectorAll = function (selector: string) {
          const matches: any[] = [];
          function search(node: any) {
            if (!node) return;
            const isMatch = (sel: string, target: any) => {
              if (sel.startsWith("[")) {
                const attr = sel.slice(1, -1);
                if (attr.includes("=")) {
                  const [k, v] = attr.split("=");
                  const cleanV = v.replace(/['"]/g, "");
                  return target.getAttribute?.(k) === cleanV || target.attributes?.[k] === cleanV;
                }
                return target.getAttribute?.(attr) !== null || target.attributes?.[attr] !== undefined;
              }
              if (sel.startsWith("#")) return target.id === sel.slice(1) || target.attributes?.id === sel.slice(1);
              if (sel.startsWith(".")) return target.className?.includes(sel.slice(1));
              return target.tagName && target.tagName.toLowerCase() === sel.toLowerCase();
            };
            for (const child of node.childNodes || node.children || []) {
              if (isMatch(selector, child)) matches.push(child);
              search(child);
            }
          }
          search(this);
          return matches;
        };
        (el as any).querySelector = function (selector: string) {
          return (el as any).querySelectorAll.call(this, selector)[0] ?? null;
        };
      }
      if (typeof (el as any).closest !== "function") {
        (el as any).closest = function (sel: string) {
          let curr: any = this;
          while (curr) {
            if (sel.startsWith("#") && (curr.id === sel.slice(1) || curr.attributes?.id === sel.slice(1))) return curr;
            if (curr.tagName && curr.tagName.toLowerCase() === sel.toLowerCase()) return curr;
            curr = curr.parentNode;
          }
          return null;
        };
      }
      if (el.style && typeof el.style.setProperty !== "function") {
        el.style.setProperty = (k: string, v: string) => {
          (el.style as any)[k] = String(v);
        };
        el.style.removeProperty = (k: string): string => {
          const prev = (el.style as any)[k] ?? "";
          delete (el.style as any)[k];
          return prev;
        };
        el.style.getPropertyValue = (k: string) => {
          return (el.style as any)[k] ?? "";
        };
      }
    }
    return el;
  };
}

const storageMap = new Map<string, string>();
const fakeLocalStorage = {
  getItem: (key: string) => storageMap.get(key) ?? null,
  setItem: (key: string, val: string) => storageMap.set(key, String(val)),
  removeItem: (key: string) => storageMap.delete(key),
  clear: () => storageMap.clear(),
};
Object.defineProperty(globalThis, "localStorage", {
  value: fakeLocalStorage,
  writable: true,
  configurable: true,
});
if (typeof window === "undefined") {
  (globalThis as any).window = globalThis;
}
Object.defineProperty(window, "localStorage", {
  value: fakeLocalStorage,
  writable: true,
  configurable: true,
});

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { SettingsPage } from "./SettingsPage";

describe("SettingsPage Sharing policy controls", () => {
  beforeEach(() => {
    getHostStatusCalls = 0;
    setAutoApproveCalls = [];
    setHostPolicyCalls = [];
    listPairingsCalls = 0;
    currentHostStatus = {
      running: true,
      ip: "192.168.1.100",
      port: 19730,
      pin: "123456",
      auto_approve: false,
      allow_input: true,
      allow_clipboard: true,
    };
  });

  const mockConnection = {
    snapshot: () => ({ busy: false }),
    subscribe: () => () => {},
    connect: async () => {},
  } as any;

  it("renders sharing toggles and binds to HostStatus", async () => {
    if (typeof document !== "undefined" && typeof document.createElement === "function") {
      const container = document.createElement("div");
      const root = createRoot(container);

      await act(async () => {
        root.render(React.createElement(SettingsPage, { connection: mockConnection }));
      });

      const inputToggle = container.querySelector("#toggle-allow-input") as HTMLButtonElement | null;
      const clipboardToggle = container.querySelector("#toggle-allow-clipboard") as HTMLButtonElement | null;
      const autoApproveToggle = container.querySelector("#toggle-auto-approve") as HTMLButtonElement | null;

      expect(inputToggle).not.toBeNull();
      expect(clipboardToggle).not.toBeNull();
      expect(autoApproveToggle).not.toBeNull();

      expect(inputToggle?.getAttribute("aria-checked")).toBe("true");
      expect(clipboardToggle?.getAttribute("aria-checked")).toBe("true");
      expect(autoApproveToggle?.getAttribute("aria-checked")).toBe("false");

      act(() => {
        root.unmount();
      });
    }
  });

  it("clicking toggle-allow-input calls setHostPolicy with both allowInput and allowClipboard", async () => {
    if (typeof document !== "undefined" && typeof document.createElement === "function") {
      const container = document.createElement("div");
      const root = createRoot(container);

      await act(async () => {
        root.render(React.createElement(SettingsPage, { connection: mockConnection }));
      });

      const inputToggle = container.querySelector("#toggle-allow-input") as HTMLButtonElement | null;
      expect(inputToggle).not.toBeNull();

      await act(async () => {
        inputToggle?.click();
      });

      expect(setHostPolicyCalls.length).toBe(1);
      expect(setHostPolicyCalls[0]).toEqual({
        allowInput: false,
        allowClipboard: true,
      });

      expect(inputToggle?.getAttribute("aria-checked")).toBe("false");

      act(() => {
        root.unmount();
      });
    }
  });

  it("clicking toggle-allow-clipboard calls setHostPolicy with both allowInput and allowClipboard", async () => {
    if (typeof document !== "undefined" && typeof document.createElement === "function") {
      const container = document.createElement("div");
      const root = createRoot(container);

      await act(async () => {
        root.render(React.createElement(SettingsPage, { connection: mockConnection }));
      });

      const clipboardToggle = container.querySelector("#toggle-allow-clipboard") as HTMLButtonElement | null;
      expect(clipboardToggle).not.toBeNull();

      await act(async () => {
        clipboardToggle?.click();
      });

      expect(setHostPolicyCalls.length).toBe(1);
      expect(setHostPolicyCalls[0]).toEqual({
        allowInput: true,
        allowClipboard: false,
      });

      expect(clipboardToggle?.getAttribute("aria-checked")).toBe("false");

      act(() => {
        root.unmount();
      });
    }
  });
});
