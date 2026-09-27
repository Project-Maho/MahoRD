import { useState, useEffect, useRef, useCallback } from "react";
import {
  Check,
  ChevronDown,
  CircleAlert,
  House,
  Maximize2,
  Minimize2,
  Monitor,
  Power,
} from "lucide-react";
import { SessionCanvas } from "./SessionCanvas";
import { SessionSettingsPanel } from "./SessionSettingsPanel";

import { useRemoteInput } from "./useRemoteInput";
import {
  pollFrameRaw,
  agentReleaseAll,
} from "@/lib/ipc";
import {
  createOverlayState,
} from "@/lib/overlay";
import type { ConnectionInstance, ConnectionSnapshot } from "@/lib/connection";

export interface SessionViewProps {
  connection?: ConnectionInstance;
  snapshot?: ConnectionSnapshot;
  state?: ConnectionSnapshot;
  pollFrame?: () => Promise<ArrayBuffer | null>;
  onDisconnect?: () => void;
  onHome?: () => void;
  onError?: (message: string) => void;
}

function getDomElement(id: string): HTMLElement | null {
  if (typeof document === "undefined" || typeof document.getElementById !== "function") {
    return null;
  }
  return document.getElementById(id);
}

export function SessionView({
  connection,
  snapshot: propSnapshot,
  state: propState,
  pollFrame: propPollFrame,
  onDisconnect,
  onHome,
  onError,
}: SessionViewProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);

  // Derive initial snapshot from prop or connection instance
  const [snapshot, setSnapshot] = useState<ConnectionSnapshot>(() => {
    return (
      propSnapshot ??
      propState ??
      connection?.snapshot() ?? {
        phase: "streaming",
        host: null,
        fieldErrors: {},
        error: null,
        cleanupError: null,
        stats: null,
        statsStatus: "idle",
        statsError: null,
        generation: 0,
        busy: false,
      }
    );
  });

  // Sync snapshot from connection instance or prop changes
  useEffect(() => {
    if (!connection) return;
    setSnapshot(connection.snapshot());
    return connection.subscribe(setSnapshot);
  }, [connection]);

  useEffect(() => {
    const next = propSnapshot ?? propState;
    if (next) {
      setSnapshot(next);
    }
  }, [propSnapshot, propState]);

  // Overlay state ported model from @/lib/overlay
  const overlayStateRef = useRef(createOverlayState());
  const [expanded, setExpanded] = useState(() => overlayStateRef.current.getExpanded());
  const [fullscreenActive, setFullscreenActive] = useState(() =>
    overlayStateRef.current.getFullscreenActive()
  );

  useEffect(() => {
    return overlayStateRef.current.subscribe((snap) => {
      setExpanded(snap.expanded);
      setFullscreenActive(snap.fullscreenActive);
    });
  }, []);

  // Stats refresh cadence (500ms like legacy)
  useEffect(() => {
    if (!connection) return;
    const timer = setInterval(() => {
      connection.refreshStats().catch(() => {});
    }, 500);
    return () => clearInterval(timer);
  }, [connection]);

  // Track FPS calculation locally based on polled frames
  const frameCountRef = useRef(0);
  const lastFpsCalcTimeRef = useRef(
    typeof performance !== "undefined" ? performance.now() : Date.now()
  );
  const [fps, setFps] = useState<number | null>(null);
  const [renderError, setRenderError] = useState<string | null>(null);

  const handleCanvasError = useCallback(
    (message: string) => {
      setRenderError(message);
      onError?.(message);
    },
    [onError]
  );

  const handlePollFrame = useCallback(async () => {
    if (propPollFrame) {
      return propPollFrame();
    }
    const buf = await pollFrameRaw();
    if (buf && buf.byteLength >= 16) {
      if (connection && !renderError) {
        connection.markFrameRendered(connection.token());
      }
      frameCountRef.current++;
      const now = typeof performance !== "undefined" ? performance.now() : Date.now();
      const elapsed = now - lastFpsCalcTimeRef.current;
      if (elapsed >= 1000) {
        setFps(Math.round((frameCountRef.current * 1000) / elapsed));
        frameCountRef.current = 0;
        lastFpsCalcTimeRef.current = now;
      }
    }
    return buf;
  }, [connection, propPollFrame, renderError]);

  const isConnected = snapshot.phase === "streaming";

  // Forward remote input through useRemoteInput hook
  useRemoteInput({
    isConnected,
    onEscape: () => {
      if (overlayStateRef.current.getExpanded()) {
        overlayStateRef.current.setExpanded(false);
        const btnExpand = getDomElement("btn-expand");
        btnExpand?.focus();
      }
    },
  });

  // Track fullscreen state and host-level release on teardown
  useEffect(() => {
    const handleFullscreenChange = () => {
      if (typeof document !== "undefined") {
        overlayStateRef.current.setFullscreenActive(Boolean(document.fullscreenElement));
      }
    };

    if (typeof document !== "undefined") {
      document.addEventListener("fullscreenchange", handleFullscreenChange);
    }

    return () => {
      if (typeof document !== "undefined") {
        document.removeEventListener("fullscreenchange", handleFullscreenChange);
      }
      agentReleaseAll().catch(() => {});
    };
  }, []);

  const handleHome = () => {
    if (onHome) {
      onHome();
    } else if (connection) {
      connection.disconnect();
    }
  };

  const handleDisconnect = () => {
    if (onDisconnect) {
      onDisconnect();
    } else if (connection) {
      connection.disconnect();
    }
  };

  const handleToggleExpand = () => {
    overlayStateRef.current.toggleExpanded();
  };

  const handleToggleFullscreen = async () => {
    try {
      if (typeof document !== "undefined" && document.fullscreenElement) {
        await document.exitFullscreen();
      } else if (containerRef.current) {
        await containerRef.current.requestFullscreen();
      }
    } catch (err) {
      console.error("Fullscreen toggle failed:", err);
    }
  };

  const hostName = snapshot.host?.name || snapshot.host?.ip || "Remote host";
  const ms = (val?: number | null) => (val == null ? "—" : `${val.toFixed(1)} ms`);

  const fpsValue = fps != null && snapshot.phase === "streaming" ? String(fps) : "—";
  const latValue =
    snapshot.stats?.latency_p50_ms != null ? ms(snapshot.stats.latency_p50_ms) : "—";

  const showConnectingModal =
    snapshot.phase === "connecting" || snapshot.phase === "waiting-video";
  const isActive =
    snapshot.phase === "waiting-video" || snapshot.phase === "streaming";
  const isWaiting = snapshot.phase === "waiting-video";
  const viaRelay =
    typeof window !== "undefined" &&
    Boolean(window.localStorage) &&
    Boolean(window.localStorage.getItem("maho-relay-url"));

  const statusDot =
    snapshot.phase === "streaming"
      ? "bg-success animate-status-pulse"
      : showConnectingModal
        ? "bg-warning"
        : "bg-subtle-foreground";

  const focusRing =
    "outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring";
  const glassIcon = `inline-flex size-8 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors duration-150 hover:bg-white/[0.08] hover:text-foreground ${focusRing}`;

  const metrics: Array<{ label: string; id?: string; value: string }> = [
    { label: "Rendered FPS", value: fpsValue },
    { label: "Decode p50", id: "session-stat-p50", value: ms(snapshot.stats?.latency_p50_ms) },
    { label: "Decode p99", id: "session-stat-p99", value: ms(snapshot.stats?.latency_p99_ms) },
    {
      label: "Frames received",
      id: "session-stat-frames",
      value: snapshot.stats?.frames_received != null ? String(snapshot.stats.frames_received) : "—",
    },
    {
      label: "Frames decoded",
      id: "session-stat-decoded",
      value: snapshot.stats?.frames_decoded != null ? String(snapshot.stats.frames_decoded) : "—",
    },
    {
      label: "Audio packets",
      id: "session-stat-audio",
      value:
        snapshot.stats?.audio_packets_received != null
          ? String(snapshot.stats.audio_packets_received)
          : "—",
    },
  ];

  return (
    <div
      ref={containerRef}
      id="viewport-container"
      className="fixed inset-0 w-full h-full overflow-hidden bg-black select-none z-[1000]"
      data-phase={snapshot.phase}
    >
      <div
        id="session-overlay"
        data-ui-scope
        role="group"
        aria-label="Session controls"
        className="@container absolute top-3 left-1/2 -translate-x-1/2 z-[1100] flex flex-col items-center gap-2 pointer-events-none w-[min(44rem,calc(100%-24px))]"
      >
        <div
          id="launcher-bar"
          className="launcher-bar surface-glass pointer-events-auto flex w-full min-h-12 items-center gap-1 rounded-[14px] p-1.5 pl-3"
        >
          <div className="overlay-badge flex min-w-0 flex-1 items-center gap-2.5">
            <span aria-hidden="true" className={`size-2 shrink-0 rounded-full ${statusDot}`} />
            <span
              id="session-host-name"
              className="truncate text-[13px] font-semibold text-foreground"
              title={hostName}
            >
              {hostName}
            </span>
            {viaRelay ? (
              <span
                id="session-via-relay"
                className="via-relay-badge inline-flex h-5 shrink-0 items-center rounded-full bg-primary/12 px-2 text-[11px] font-medium text-brand-soft ring-1 ring-inset ring-primary/25"
              >
                via relay
              </span>
            ) : null}
          </div>

          <div className="overlay-stats flex shrink-0 items-center gap-4 px-2 text-xs tabular-nums @max-[34rem]:hidden">
            <span id="session-stat-fps" className="text-muted-foreground">
              Rendered FPS{" "}
              <span className="font-mono font-medium text-foreground">{fpsValue}</span>
            </span>
            <span id="session-stat-lat" className="text-muted-foreground">
              Decode p50{" "}
              <span className="font-mono font-medium text-foreground">{latValue}</span>
            </span>
          </div>

          <span aria-hidden="true" className="mx-1 h-5 w-px shrink-0 bg-white/10" />

          <button
            type="button"
            id="btn-home"
            className={`overlay-btn ${glassIcon}`}
            title="Home — end session and return to main screen"
            aria-label="Home — end session and return to main screen"
            onClick={handleHome}
          >
            <House className="size-4" aria-hidden="true" />
          </button>

          <button
            type="button"
            id="btn-expand"
            className={`overlay-btn ${glassIcon} ${expanded ? "bg-white/[0.08] text-foreground" : ""}`}
            aria-expanded={expanded}
            aria-controls="launcher-panel"
            title="More session controls"
            aria-label="More session controls"
            onClick={handleToggleExpand}
          >
            <ChevronDown
              className={`size-4 transition-transform duration-200 ${expanded ? "rotate-180" : ""}`}
              aria-hidden="true"
            />
          </button>

          <button
            type="button"
            id="btn-disconnect"
            className={`overlay-btn btn-disconnect inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs font-semibold text-destructive transition-colors duration-150 hover:bg-destructive/12 ${focusRing}`}
            title="Disconnect"
            onClick={handleDisconnect}
          >
            <Power className="size-3.5" aria-hidden="true" />
            Disconnect
          </button>
        </div>

        {expanded && (
          <div
            id="launcher-panel"
            role="region"
            aria-label="Session details"
            className="launcher-panel surface-glass pointer-events-auto flex w-[340px] max-w-full max-h-[calc(100dvh-96px)] flex-col gap-4 overflow-y-auto rounded-[14px] p-4 text-foreground animate-panel-in"
          >
            <div className="flex items-center justify-between">
              <h2 className="text-[13px] font-semibold">Session details</h2>
              <span className="truncate pl-3 font-mono text-[11.5px] text-muted-foreground">
                {snapshot.host?.ip ?? ""}
              </span>
            </div>

            {snapshot.statsError && (
              <p
                id="session-stats-notice"
                role="status"
                className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/10 px-3 py-2 text-xs text-destructive"
              >
                <CircleAlert className="mt-px size-3.5 shrink-0" aria-hidden="true" />
                <span>{snapshot.statsError}</span>
              </p>
            )}

            <dl className="grid grid-cols-2 gap-2">
              <div className="col-span-2 flex items-center justify-between rounded-lg bg-white/[0.04] px-3 py-2.5 ring-1 ring-inset ring-white/[0.06]">
                <dt className="text-[11px] font-medium text-muted-foreground">State</dt>
                <dd className="flex items-center gap-2 text-[13px] font-medium">
                  <span aria-hidden="true" className={`size-1.5 rounded-full ${statusDot}`} />
                  <span id="session-stat-state" className="capitalize">
                    {snapshot.phase}
                  </span>
                </dd>
              </div>
              {metrics.map((m) => (
                <div
                  key={m.label}
                  className="rounded-lg bg-white/[0.04] px-3 py-2.5 ring-1 ring-inset ring-white/[0.06]"
                >
                  <dt className="text-[11px] font-medium text-muted-foreground">{m.label}</dt>
                  <dd
                    id={m.id}
                    className="mt-0.5 font-mono text-[15px] font-medium leading-5 tabular-nums"
                  >
                    {m.value}
                  </dd>
                </div>
              ))}
            </dl>

            <div className="h-px bg-white/[0.06]" />

            <SessionSettingsPanel
              isConnected={snapshot.phase === "streaming"}
              onError={onError}
            />

            <div className="h-px bg-white/[0.06]" />

            <p className="text-[11.5px] leading-4 text-muted-foreground">
              Rendered FPS counts frames drawn by this client. Decode is local
              receive-to-decode time, not network latency.
            </p>

            <button
              type="button"
              id="btn-fullscreen"
              className={`surface-raised inline-flex h-9 w-full items-center justify-center gap-2 rounded-lg text-[13px] font-medium text-foreground transition-colors duration-150 hover:bg-surface-4 ${focusRing}`}
              aria-pressed={fullscreenActive}
              title="Toggle fullscreen"
              onClick={handleToggleFullscreen}
            >
              {fullscreenActive ? (
                <Minimize2 className="size-4" aria-hidden="true" />
              ) : (
                <Maximize2 className="size-4" aria-hidden="true" />
              )}
              <span id="btn-fullscreen-label">
                {fullscreenActive ? "Exit Fullscreen" : "Fullscreen"}
              </span>
            </button>
          </div>
        )}
      </div>

      {showConnectingModal && (
        <div
          id="session-connecting-modal"
          data-ui-scope
          role="dialog"
          aria-modal="true"
          aria-labelledby="modal-connecting-text"
          className="absolute inset-0 z-[1200] flex items-center justify-center bg-black/65 backdrop-blur-md pointer-events-auto animate-fade-in"
        >
          <div className="connection-panel surface-glass flex w-[min(360px,calc(100%-32px))] flex-col items-center gap-4 rounded-2xl p-6 text-center animate-panel-in">
            <div className="relative grid size-14 place-items-center">
              <div
                className="spinner absolute inset-0 rounded-full border-2 border-white/10 border-t-primary animate-spin motion-reduce:animate-none"
                aria-hidden="true"
              />
              <Monitor className="size-5 text-brand-soft" aria-hidden="true" />
            </div>
            <div className="space-y-1">
              <div
                id="modal-connecting-text"
                role="status"
                className="text-[15px] font-semibold text-foreground"
              >
                {isWaiting ? "Waiting for video" : "Connecting"}
              </div>
              <p id="modal-host-name" className="font-mono text-xs text-muted-foreground">
                {snapshot.host?.name || snapshot.host?.ip || ""}
              </p>
            </div>
            <p className="text-[13px] text-muted-foreground">
              {isWaiting
                ? "Connected. Waiting for the first frame."
                : "Establishing a secure session"}
            </p>
            <ol className="flex items-center gap-2 text-xs font-medium" aria-label="Connection progress">
              <li
                className={`inline-flex h-6 items-center gap-1.5 rounded-full px-2.5 ring-1 ring-inset ${
                  isWaiting
                    ? "bg-success/10 text-success ring-success/20"
                    : "bg-primary/12 text-brand-soft ring-primary/25"
                }`}
              >
                {isWaiting ? <Check className="size-3" aria-hidden="true" /> : null}
                Connect
              </li>
              <li aria-hidden="true" className="h-px w-4 bg-white/15" />
              <li
                className={`inline-flex h-6 items-center rounded-full px-2.5 ring-1 ring-inset ${
                  isWaiting
                    ? "bg-primary/12 text-brand-soft ring-primary/25"
                    : "bg-white/[0.05] text-muted-foreground ring-white/[0.06]"
                }`}
              >
                First frame
              </li>
            </ol>
            <button
              id="btn-cancel-connect"
              type="button"
              className={`surface-raised mt-1 inline-flex h-9 items-center justify-center rounded-lg px-4 text-[13px] font-medium text-foreground transition-colors duration-150 hover:bg-surface-4 ${focusRing}`}
              onClick={handleDisconnect}
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {renderError && (
        <div
          role="alert"
          id="render-error-alert"
          className="absolute top-[76px] left-1/2 -translate-x-1/2 z-[1050] flex max-w-[calc(100%-32px)] items-start gap-2 rounded-lg border border-destructive/30 bg-[#281012]/95 px-3.5 py-2.5 text-[13px] font-medium text-[#ffb4b4] shadow-float backdrop-blur-md"
        >
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>{renderError}</span>
        </div>
      )}

      <SessionCanvas
        active={isActive && !renderError}
        pollFrame={handlePollFrame}
        onError={handleCanvasError}
      />
    </div>
  );
}
