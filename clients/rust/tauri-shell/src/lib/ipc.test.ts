import { describe, expect, it, beforeEach, afterEach, mock } from "bun:test";
import {
  MAHO_COMMANDS,
  isNativeAvailable,
  invokeCommand,
  listHosts,
  connect,
  getCursorPosition,
  setBitrate,
  pollFrameRaw,
  disconnect,
  audioStatus,
  listAudioDevices,
  setAudioVolume,
  setAudioMuted,
  setAudioDevice,
  listPairings,
  forgetPairing,
  stats,
  sendInput,
  agentExecuteAction,
  agentGetScreenInfo,
  agentCaptureScreen,
  agentReleaseAll,
  getHostStatus,
  startHost,
  stopHost,
} from "./ipc";
import type {
  HostItem,
  PairingSummary,
  SessionStats,
  HostStatus,
  CursorState,
  ScreenInfo,
  DesktopAudioStatus,
  InputPayload,
  AgentAction,
} from "./ipc";

const ALL_22_COMMANDS = [
  "list_hosts",
  "connect",
  "get_cursor_position",
  "set_bitrate",
  "poll_frame_raw",
  "disconnect",
  "audio_status",
  "list_audio_devices",
  "set_audio_volume",
  "set_audio_muted",
  "set_audio_device",
  "list_pairings",
  "forget_pairing",
  "stats",
  "send_input",
  "agent_execute_action",
  "agent_get_screen_info",
  "agent_capture_screen",
  "agent_release_all",
  "get_host_status",
  "start_host",
  "stop_host",
] as const;

const DYNAMIC_7_COMMANDS = [
  "start_host",
  "stop_host",
  "audio_status",
  "list_audio_devices",
  "set_audio_volume",
  "set_audio_muted",
  "set_audio_device",
] as const;

describe("MAHO_COMMANDS registry", () => {
  it("has exact length of 22 commands", () => {
    expect(MAHO_COMMANDS.length).toBe(22);
  });

  it("contains all 22 authoritative backend commands", () => {
    for (const cmd of ALL_22_COMMANDS) {
      expect(MAHO_COMMANDS).toContain(cmd);
    }
  });

  it("contains all 7 dynamically invoked commands", () => {
    for (const cmd of DYNAMIC_7_COMMANDS) {
      expect(MAHO_COMMANDS).toContain(cmd);
    }
  });
});

describe("isNativeAvailable", () => {
  const originalTauri = (globalThis as any).window?.__TAURI__;

  afterEach(() => {
    if (typeof (globalThis as any).window !== "undefined") {
      if (originalTauri !== undefined) {
        (globalThis as any).window.__TAURI__ = originalTauri;
      } else {
        delete (globalThis as any).window.__TAURI__;
      }
    }
  });

  it("returns false when window.__TAURI__ is not present", () => {
    if (typeof (globalThis as any).window === "undefined") {
      (globalThis as any).window = {};
    }
    delete (globalThis as any).window.__TAURI__;
    expect(isNativeAvailable()).toBe(false);
  });

  it("returns true when window.__TAURI__ exists", () => {
    if (typeof (globalThis as any).window === "undefined") {
      (globalThis as any).window = {};
    }
    (globalThis as any).window.__TAURI__ = {};
    expect(isNativeAvailable()).toBe(true);
  });
});

describe("invokeCommand", () => {
  afterEach(() => {
    if (typeof (globalThis as any).window !== "undefined") {
      delete (globalThis as any).window.__TAURI__;
    }
  });

  it("throws clear error when Tauri is not available", async () => {
    if (typeof (globalThis as any).window !== "undefined") {
      delete (globalThis as any).window.__TAURI__;
    }
    expect(invokeCommand("list_hosts")).rejects.toThrow(
      /Tauri IPC is not available/
    );
  });

  it("dispatches to window.__TAURI__.core.invoke", async () => {
    const mockInvoke = mock(async (cmd: string, args?: Record<string, unknown>) => {
      return { echoed: cmd, args };
    });
    (globalThis as any).window = {
      __TAURI__: {
        core: {
          invoke: mockInvoke,
        },
      },
    };

    const res = await invokeCommand<{ echoed: string; args?: any }>("list_hosts", {
      test: 123,
    });
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("list_hosts", { test: 123 });
    expect(res).toEqual({ echoed: "list_hosts", args: { test: 123 } });
  });

  it("falls back to window.__TAURI__.invoke when core.invoke is absent", async () => {
    const mockInvoke = mock(async (cmd: string, args?: Record<string, unknown>) => {
      return { direct: cmd, args };
    });
    (globalThis as any).window = {
      __TAURI__: {
        invoke: mockInvoke,
      },
    };

    const res = await invokeCommand<{ direct: string; args?: any }>("get_cursor_position");
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("get_cursor_position", undefined);
    expect(res).toEqual({ direct: "get_cursor_position", args: undefined });
  });

  it("falls back to window.__TAURI__.tauri", async () => {
    const mockInvoke = mock(async (cmd: string, args?: Record<string, unknown>) => {
      return { fallback: cmd, args };
    });
    (globalThis as any).window = {
      __TAURI__: {
        tauri: mockInvoke,
      },
    };

    const res = await invokeCommand<{ fallback: string; args?: any }>("stats");
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("stats", undefined);
    expect(res).toEqual({ fallback: "stats", args: undefined });
  });
});

describe("named thin wrappers", () => {
  let invoked: { cmd: string; args?: Record<string, unknown> }[] = [];

  const mockHost: HostItem = {
    id: "host-1",
    name: "Desktop Host",
    ip: "10.0.0.1",
    os: "linux",
    online: true,
    paired: false,
  };
  const mockCursor: CursorState = {
    x: 100,
    y: 200,
    cursor_type: 1,
  };
  const mockAudioStatus: DesktopAudioStatus = {
    active: true,
    volume: 0.8,
    muted: false,
    devices: [],
    consumed_samples: 1024,
  };
  const mockPairing: PairingSummary = {
    id: "pair-1",
    hostName: "Desktop Host",
    addedAtUnixMs: 1700000000000,
  };
  const mockStats: SessionStats = {
    connected: true,
    state: "connected",
    frames_received: 120,
    frames_decoded: 120,
    audio_packets_received: 60,
  };
  const mockScreenInfo: ScreenInfo = {
    width: 1920,
    height: 1080,
    scale: 1,
    monitors: [],
    connected_host: "host-1",
  };
  const mockHostStatus: HostStatus = {
    running: true,
    ip: "127.0.0.1",
    port: 19730,
    pin: "1234",
    auto_approve: false,
  };

  beforeEach(() => {
    invoked = [];
    (globalThis as any).window = {
      __TAURI__: {
        core: {
          invoke: async (cmd: string, args?: Record<string, unknown>) => {
            invoked.push({ cmd, args });
            switch (cmd) {
              case "list_hosts":
                return [mockHost];
              case "get_cursor_position":
                return mockCursor;
              case "audio_status":
              case "list_audio_devices":
              case "set_audio_volume":
              case "set_audio_muted":
              case "set_audio_device":
                return mockAudioStatus;
              case "list_pairings":
                return [mockPairing];
              case "stats":
                return mockStats;
              case "agent_get_screen_info":
                return mockScreenInfo;
              case "get_host_status":
              case "start_host":
              case "stop_host":
                return mockHostStatus;
              default:
                return "ok";
            }
          },
        },
      },
    };
  });

  afterEach(() => {
    delete (globalThis as any).window.__TAURI__;
  });

  it("calls each thin wrapper and dispatches expected command and args", async () => {
    const hosts: HostItem[] = await listHosts();
    expect(hosts).toEqual([mockHost]);
    expect(invoked[invoked.length - 1]).toEqual({ cmd: "list_hosts", args: undefined });

    await connect({ host: "10.0.0.1", tcpPort: 19730 });
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "connect",
      args: { host: "10.0.0.1", tcpPort: 19730 },
    });

    const cursor: CursorState = await getCursorPosition();
    expect(cursor).toEqual(mockCursor);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "get_cursor_position",
      args: undefined,
    });

    await setBitrate(50);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "set_bitrate",
      args: { bitrateMbps: 50 },
    });

    await pollFrameRaw();
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "poll_frame_raw",
      args: undefined,
    });

    await disconnect();
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "disconnect",
      args: undefined,
    });

    const audio: DesktopAudioStatus = await audioStatus();
    expect(audio).toEqual(mockAudioStatus);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "audio_status",
      args: undefined,
    });

    await listAudioDevices();
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "list_audio_devices",
      args: undefined,
    });

    await setAudioVolume(0.8);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "set_audio_volume",
      args: { volume: 0.8 },
    });

    await setAudioMuted(true);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "set_audio_muted",
      args: { muted: true },
    });

    await setAudioDevice("output-0");
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "set_audio_device",
      args: { deviceId: "output-0" },
    });

    const pairings: PairingSummary[] = await listPairings();
    expect(pairings).toEqual([mockPairing]);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "list_pairings",
      args: undefined,
    });

    await forgetPairing("pair-123");
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "forget_pairing",
      args: { id: "pair-123" },
    });

    const sessionStats: SessionStats = await stats();
    expect(sessionStats).toEqual(mockStats);
    expect(invoked[invoked.length - 1]).toEqual({ cmd: "stats", args: undefined });

    const inputEvt: InputPayload = { event_type: "MouseMove", x: 0.5, y: 0.5 };
    await sendInput(inputEvt);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "send_input",
      args: { event: inputEvt as any },
    });

    const agentAction: AgentAction = { action: "mouse_move", x: 0.5, y: 0.5 };
    await agentExecuteAction(agentAction);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "agent_execute_action",
      args: { action: agentAction as any },
    });

    const screenInfo: ScreenInfo = await agentGetScreenInfo();
    expect(screenInfo).toEqual(mockScreenInfo);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "agent_get_screen_info",
      args: undefined,
    });

    await agentCaptureScreen("jpeg");
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "agent_capture_screen",
      args: { format: "jpeg" },
    });

    await agentReleaseAll();
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "agent_release_all",
      args: undefined,
    });

    const hostStatus: HostStatus = await getHostStatus();
    expect(hostStatus).toEqual(mockHostStatus);
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "get_host_status",
      args: undefined,
    });

    await startHost();
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "start_host",
      args: undefined,
    });

    await stopHost();
    expect(invoked[invoked.length - 1]).toEqual({
      cmd: "stop_host",
      args: undefined,
    });
  });
});
