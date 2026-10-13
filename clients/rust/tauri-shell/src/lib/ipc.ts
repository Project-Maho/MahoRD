/**
 * Single typed boundary through which the entire frontend calls Tauri.
 * Registry of backend commands used by the desktop frontend.
 */

// =============================================================================
// Authoritative Command Registry
// =============================================================================

export const MAHO_COMMANDS = [
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
  "hide_host",
  "unhide_host",
  "list_hidden_hosts",
  "set_auto_approve",
  "set_host_policy",
  "set_stream_options",
  "get_stream_state",
  "cancel_connect", "set_client_clipboard", "request_secure_attention",
  "host_guests", "host_approve", "host_deny", "host_kick", "host_kick_all",
  "host_guest_permissions", "wake_computer", "start_key_capture", "stop_key_capture", "sync_tray_state",
] as const;

export type MahoCommand = (typeof MAHO_COMMANDS)[number];

// =============================================================================
// Data Interfaces Derived from lib.rs and Core Crates
// =============================================================================

/**
 * Discovered host representation from LAN multicast or Tailscale daemon.
 */
export interface HostItem {
  id: string;
  name: string;
  ip: string;
  os: string;
  online: boolean;
  paired: boolean;
  last_seen?: string | null;
  tcp_port?: number | null;
  udp_port?: number | null;
}

/**
 * Persisted network endpoint information for a paired host.
 */
export interface PairingEndpoint {
  host: string;
  tcpPort: number;
  udpPort: number;
}

/**
 * Pairing summary persisted in client pairing store.
 */
export interface PairingSummary {
  id: string;
  hostName: string;
  addedAtUnixMs: number;
  lastEndpoint?: PairingEndpoint | null;
}

/**
 * Result returned upon successful session authentication and handshake.
 */
export interface ConnectResponse {
  secure_attention: boolean;
  clipboard_image: boolean;
  pairing_id: string;
  host_name: string;
  server_name: string;
}

/**
 * Parameters accepted by the connect command.
 */
export interface ConnectArgs {
  host: string;
  tcpPort?: number | null;
  udpPort?: number | null;
  pin?: string | null;
  pairingId?: string | null;
}

/**
 * Structured IPC error returned by connection or pairing commands.
 */
export interface IpcError {
  code: string;
  message: string;
  stage: string;
  retryable: boolean;
}

/**
 * Session telemetry and latency statistics.
 */
export interface SessionStats {
  connected: boolean;
  state: string;
  frames_received: number;
  frames_decoded: number;
  audio_packets_received: number;
  latency_p50_ms?: number | null;
  latency_p99_ms?: number | null;
  rtt_latest_ms?: number | null;
  rtt_smoothed_ms?: number | null;
  measured_mbps?: number | null;
  packet_loss_ratio?: number | null;
  decoder_backend?: string | null;
  session_info?: SessionInfo | null;
}

export interface GuestPermissions {
  keyboard: boolean; mouse: boolean; gamepad: boolean; clipboard: boolean; audio: boolean;
}
export interface SessionInfo {
  guest_names: string[] | null;
  permissions: GuestPermissions | null;
  ended: string | null;
  trust: string | null;
  reconnect: "idle" | "reconnecting" | "reconnected" | "gave-up";
  attempt: number | null;
  next_retry_ms: number | null;
  reason: string | null;
  sas_request_id: number | null;
  sas_result: string | null;
}
export interface HostGuests {
  sessions: { id: string; pairing_id: string; name: string; trust: string; connected_seconds: number; permissions: GuestPermissions | null }[];
  pending: { id: string; name: string; expires_in_ms: number }[];
  events: string[];
  error: string | null;
}
export const hostGuests = (drainEvents = true) => invokeCommand<HostGuests>("host_guests", { drainEvents });
export const hostApprove = (id: string, owner: boolean) => invokeCommand<boolean>("host_approve", { id, owner });
export const hostDeny = (id: string) => invokeCommand<boolean>("host_deny", { id });
export const hostKick = (id: string) => invokeCommand<boolean>("host_kick", { id });
export const hostKickAll = () => invokeCommand<number>("host_kick_all");
export const hostGuestPermissions = (pairingId: string, permissions: GuestPermissions) => invokeCommand<void>("host_guest_permissions", { pairingId, permissions });
export const wakeComputer = (mac: string) => invokeCommand<void>("wake_computer", { mac });
export const setClientClipboard = (enabled: boolean) => invokeCommand<void>("set_client_clipboard", { enabled });
export const requestSecureAttention = () => invokeCommand<number>("request_secure_attention");

/**
 * Daemon status for embedded host server.
 */
export interface HostStatus {
  running: boolean;
  ip: string;
  port: number;
  pin: string;
  auto_approve: boolean;
  allow_input?: boolean;
  allow_clipboard?: boolean;
}

/**
 * Parameters for setting host input and clipboard policy.
 */
export interface SetHostPolicyArgs {
  allowInput: boolean;
  allowClipboard: boolean;
}

/**
 * Codec selection for stream options.
 */
export type StreamCodec = "auto" | "h264" | "hevc";

/**
 * Parameters for setting stream options.
 */
export interface SetStreamOptionsArgs {
  scale: number;
  fps: number;
  codec: StreamCodec;
  displayIndex: number;
}

/**
 * Display output description from host stream state.
 */
export interface StreamDisplay {
  index: number;
  name: string;
  width: number;
  height: number;
  primary: boolean;
}

/**
 * Host stream active settings confirmed by host.
 */
export interface StreamActiveState {
  width: number;
  height: number;
  fps: number;
  codec: string;
  displayIndex: number;
  bitrate?: number;
}

/**
 * Stream policy settings returned by host stream state.
 */
export interface StreamPolicyState {
  allowInput: boolean;
  allowClipboard: boolean;
}

/**
 * Current stream state including active settings, displays, policy, and rejection.
 */
export interface StreamState {
  active: StreamActiveState | null;
  displays: StreamDisplay[];
  policy: StreamPolicyState | null;
  lastReject: string | null;
  supported: boolean;
}

/**
 * Remote host cursor coordinates and display shape indicator.
 */
export interface CursorState {
  x: number;
  y: number;
  cursor_type: number;
}

/**
 * Desktop audio output device description.
 */
export interface DesktopAudioDevice {
  id: string;
  name: string;
  supported: boolean;
}

/**
 * Desktop audio playback status and available output devices.
 */
export interface DesktopAudioStatus {
  active: boolean;
  volume: number;
  muted: boolean;
  device_id?: string | null;
  devices: DesktopAudioDevice[];
  consumed_samples: number;
  error?: string | null;
}

/**
 * Individual monitor geometry within remote screen info.
 */
export interface MonitorInfo {
  id: number;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  is_primary: boolean;
}

/**
 * Remote display resolution, scaling, and monitor layout.
 */
export interface ScreenInfo {
  width: number;
  height: number;
  scale: number;
  logical_width?: number | null;
  logical_height?: number | null;
  monitors: MonitorInfo[];
  connected_host: string;
}

/**
 * Low-level keyboard, mouse, pointer, or gamepad input payload.
 */
export interface InputPayload {
  event_type: string;
  x?: number;
  y?: number;
  view_width?: number;
  view_height?: number;
  key_code?: number | null;
  modifiers?: number;
  scroll_dx?: number;
  scroll_dy?: number;
}

/**
 * Supported mouse buttons for synthetic agent actions.
 */
export type MouseButton = "left" | "right" | "middle";

/**
 * High-level AI agent input actions supported by remote-agent engine.
 */
export type AgentAction =
  | { action: "mouse_move"; x: number; y: number; normalized?: boolean }
  | { action: "mouse_down"; button?: MouseButton }
  | { action: "mouse_up"; button?: MouseButton }
  | {
      action: "click";
      x: number;
      y: number;
      button?: MouseButton;
      count?: number;
      normalized?: boolean;
    }
  | {
      action: "drag";
      start_x: number;
      start_y: number;
      end_x: number;
      end_y: number;
      button?: MouseButton;
      steps?: number;
      duration_ms?: number;
      normalized?: boolean;
    }
  | {
      action: "scroll";
      dx: number;
      dy: number;
      x?: number | null;
      y?: number | null;
      normalized?: boolean;
    }
  | { action: "key_down"; key: string }
  | { action: "key_up"; key: string }
  | { action: "key_press"; key: string; hold_ms?: number }
  | { action: "hotkey"; keys: string[] }
  | { action: "type_text"; text: string; delay_ms?: number; paste_mode?: boolean }
  | { action: "release_all" };

// =============================================================================
// Native IPC Availability & Invocation
// =============================================================================

/**
 * Checks whether native Tauri runtime bindings are present in current window context.
 */
export function isNativeAvailable(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof (window as any).__TAURI__ !== "undefined" &&
    (window as any).__TAURI__ !== null
  );
}

/**
 * The ONLY place in the codebase that touches the Tauri invoke function.
 * Resolves invoke from window.__TAURI__?.core?.invoke ?? window.__TAURI__?.invoke ?? window.__TAURI__?.tauri.
 */
export async function invokeCommand<T>(
  command: MahoCommand,
  args?: Record<string, unknown>
): Promise<T> {
  const tauri =
    typeof window !== "undefined" ? (window as any).__TAURI__ : undefined;
  const candidate =
    tauri?.core?.invoke ?? tauri?.invoke ?? tauri?.tauri;
  const invokeFn =
    typeof candidate === "function"
      ? candidate
      : typeof candidate?.invoke === "function"
        ? candidate.invoke.bind(candidate)
        : undefined;

  if (typeof invokeFn !== "function") {
    throw new Error(
      `Tauri IPC is not available: cannot invoke command '${command}'. Ensure application is running inside a Tauri shell.`
    );
  }

  return (await invokeFn(command, args)) as T;
}

// =============================================================================
// Thin Named Wrappers for Authoritative Commands
// =============================================================================

export const listHosts = () => invokeCommand<HostItem[]>("list_hosts");

export const connect = (args: ConnectArgs) =>
  invokeCommand<ConnectResponse>(
    "connect",
    args as unknown as Record<string, unknown>
  );

export const getCursorPosition = () =>
  invokeCommand<CursorState>("get_cursor_position");

export const setBitrate = (bitrateMbps: number) =>
  invokeCommand<void>("set_bitrate", { bitrateMbps });

export const pollFrameRaw = () =>
  invokeCommand<ArrayBuffer>("poll_frame_raw");

export const disconnect = () => invokeCommand<void>("disconnect");

export const audioStatus = () =>
  invokeCommand<DesktopAudioStatus>("audio_status");

export const listAudioDevices = () =>
  invokeCommand<DesktopAudioStatus>("list_audio_devices");

export const setAudioVolume = (volume: number) =>
  invokeCommand<DesktopAudioStatus>("set_audio_volume", { volume });

export const setAudioMuted = (muted: boolean) =>
  invokeCommand<DesktopAudioStatus>("set_audio_muted", { muted });

export const setAudioDevice = (deviceId: string | null) =>
  invokeCommand<DesktopAudioStatus>("set_audio_device", { deviceId });

export const listPairings = () =>
  invokeCommand<PairingSummary[]>("list_pairings");

export const forgetPairing = (id: string) =>
  invokeCommand<void>("forget_pairing", { id });

export const hideHost = (name: string) =>
  invokeCommand<void>("hide_host", { name });

export const unhideHost = (name: string) =>
  invokeCommand<void>("unhide_host", { name });

export const listHiddenHosts = () =>
  invokeCommand<string[]>("list_hidden_hosts");

export const stats = () => invokeCommand<SessionStats>("stats");

export const sendInput = (event: InputPayload) =>
  invokeCommand<void>("send_input", {
    event: event as unknown as Record<string, unknown>,
  });

export const agentExecuteAction = (action: AgentAction) =>
  invokeCommand<number>("agent_execute_action", {
    action: action as unknown as Record<string, unknown>,
  });

export const agentGetScreenInfo = () =>
  invokeCommand<ScreenInfo>("agent_get_screen_info");

export const agentCaptureScreen = (format?: string) =>
  invokeCommand<string>(
    "agent_capture_screen",
    format ? { format } : undefined
  );

export const agentReleaseAll = () =>
  invokeCommand<number>("agent_release_all");

export const getHostStatus = () =>
  invokeCommand<HostStatus>("get_host_status");

export const startHost = () => invokeCommand<HostStatus>("start_host");

export const stopHost = () => invokeCommand<HostStatus>("stop_host");

export const setAutoApprove = (enabled: boolean) =>
  invokeCommand<HostStatus>("set_auto_approve", { enabled });

export const setHostPolicy = (args: SetHostPolicyArgs) =>
  invokeCommand<HostStatus>(
    "set_host_policy",
    args as unknown as Record<string, unknown>
  );

export const setStreamOptions = (args: SetStreamOptionsArgs) =>
  invokeCommand<void>(
    "set_stream_options",
    args as unknown as Record<string, unknown>
  );

export const getStreamState = () =>
  invokeCommand<StreamState>("get_stream_state");


