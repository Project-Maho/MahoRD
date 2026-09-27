import { useState, useEffect, useCallback, useRef } from "react";
import {
  setBitrate,
  audioStatus,
  listAudioDevices,
  setAudioVolume,
  setAudioMuted,
  setAudioDevice,
  type DesktopAudioStatus,
  type DesktopAudioDevice,
} from "@/lib/ipc";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";

export const QUALITY_STORAGE_KEY = "maho-quality-mbps";

export const QUALITY_OPTIONS = [8, 25, 50, 100] as const;

export function savedBitrateMbps(): number {
  if (typeof window === "undefined" || !window.localStorage) {
    return 50;
  }
  try {
    const saved = Number(window.localStorage.getItem(QUALITY_STORAGE_KEY));
    return Number.isInteger(saved) && saved >= 1 && saved <= 300 ? saved : 50;
  } catch {
    return 50;
  }
}

/**
 * Pure logic helper: Persists bitrate to localStorage under QUALITY_STORAGE_KEY
 * and invokes setBitrate when connected.
 */
export async function applyBitrateCeiling(
  mbps: number,
  isConnected = true
): Promise<void> {
  if (typeof window !== "undefined" && window.localStorage) {
    try {
      window.localStorage.setItem(QUALITY_STORAGE_KEY, String(mbps));
    } catch (e) {
      console.error("Failed to write bitrate to localStorage:", e);
    }
  }
  if (isConnected && typeof setBitrate === "function") {
    await setBitrate(mbps);
  }
}

/**
 * Pure logic helper: Converts a 0..100 volume percentage to a 0..1 value
 * and invokes setAudioVolume when connected.
 */
export async function applyVolumeLevel(
  percent: number,
  isConnected = true
): Promise<DesktopAudioStatus | undefined> {
  if (!isConnected || typeof setAudioVolume !== "function") return;
  const volume = percent / 100;
  return await setAudioVolume(volume);
}

/**
 * Pure logic helper: Toggles audio muted status against the current muted state
 * and invokes setAudioMuted when connected.
 */
export async function toggleAudioMuted(
  currentMuted: boolean,
  isConnected = true
): Promise<DesktopAudioStatus | undefined> {
  if (!isConnected || typeof setAudioMuted !== "function") return;
  return await setAudioMuted(!currentMuted);
}

/**
 * Pure logic helper: Sets audio device (or null for default) with optional input release.
 */
export async function applyAudioDeviceSelection(
  selectedDeviceId: string | null,
  isConnected = true,
  onReleaseInputs?: () => Promise<void> | void
): Promise<DesktopAudioStatus | undefined> {
  if (!isConnected || typeof setAudioDevice !== "function") return;
  if (onReleaseInputs) {
    await onReleaseInputs();
  }
  const devId =
    !selectedDeviceId || selectedDeviceId === "default"
      ? null
      : selectedDeviceId;
  return await setAudioDevice(devId);
}

/**
 * Pure logic helper: Refreshes audio output devices.
 */
export async function refreshAudioDeviceList(
  isConnected = true
): Promise<DesktopAudioStatus | undefined> {
  if (!isConnected || typeof listAudioDevices !== "function") return;
  return await listAudioDevices();
}

export interface SessionSettingsPanelProps {
  isConnected?: boolean;
  onReleaseInputs?: () => Promise<void> | void;
  onError?: (error: string) => void;
}

export function SessionSettingsPanel({
  isConnected = true,
  onReleaseInputs,
  onError,
}: SessionSettingsPanelProps) {
  // Quality / Bitrate state
  const [bitrate, setBitrateState] = useState<number>(savedBitrateMbps);
  const [qualityError, setQualityError] = useState<string | null>(null);

  // Audio state
  const [audioState, setAudioState] = useState<DesktopAudioStatus | null>(null);
  const [audioPending, setAudioPending] = useState(false);
  const [audioError, setAudioError] = useState<string | null>(null);
  const [liveVolume, setLiveVolume] = useState<number>(100);
  const [selectedDeviceId, setSelectedDeviceId] = useState<string>("default");

  // Keep ref to avoid stale closure during async operations
  const audioStateRef = useRef<DesktopAudioStatus | null>(audioState);
  audioStateRef.current = audioState;

  const isConnectedRef = useRef(isConnected);
  isConnectedRef.current = isConnected;

  // Handle bitrate change: persists to localStorage and calls setBitrate when connected
  const handleBitrateChange = useCallback(
    async (value: string) => {
      const mbps = Number(value);
      setBitrateState(mbps);
      try {
        await applyBitrateCeiling(mbps, isConnectedRef.current);
        setQualityError(null);
      } catch (err: any) {
        const msg = `Bitrate apply failed: ${err}`;
        setQualityError(msg);
        onError?.(msg);
      }
    },
    [onError]
  );

  // Audio status helper matching legacy audioCommand
  const executeAudioCommand = useCallback(
    async (
      commandFn: () => Promise<DesktopAudioStatus | undefined>,
      isDeviceSwitch = false
    ) => {
      if (!isConnectedRef.current) return;
      setAudioPending(true);
      try {
        if (isDeviceSwitch && onReleaseInputs) {
          await onReleaseInputs();
        }
        const status = await commandFn();
        if (status) {
          setAudioState(status);
          audioStateRef.current = status;
          setAudioError(null);
          if (status.volume != null) {
            setLiveVolume(Math.round(status.volume * 100));
          }
          if (status.device_id != null) {
            setSelectedDeviceId(status.device_id || "default");
          }
          if (status.error) {
            setAudioError(status.error);
            onError?.(status.error);
          }
        }
        return status;
      } catch (err: any) {
        const errStr = String(err);
        setAudioError(errStr);
        onError?.(errStr);
        if (isDeviceSwitch && audioStateRef.current) {
          setAudioState({ ...audioStateRef.current, active: false });
        }
      } finally {
        setAudioPending(false);
      }
    },
    [onReleaseInputs, onError]
  );

  // Poll audio status periodically when connected (legacy matches 500ms interval)
  useEffect(() => {
    if (!isConnected) return;
    if (typeof audioStatus !== "function") return;

    let mounted = true;
    const fetchStatus = async () => {
      try {
        const status = await audioStatus();
        if (!mounted || !status) return;
        setAudioState(status);
        audioStateRef.current = status;
        setLiveVolume(Math.round((status.volume ?? 1) * 100));
        if (status.device_id != null) {
          setSelectedDeviceId(status.device_id || "default");
        }
        if (status.error) {
          setAudioError(status.error);
        }
      } catch (err: any) {
        // Suppress polling error in background if unmounted
        if (!mounted) return;
      }
    };

    fetchStatus();
    const interval = setInterval(fetchStatus, 500);
    return () => {
      mounted = false;
      clearInterval(interval);
    };
  }, [isConnected]);

  // Volume slider events
  const handleVolumeInput = useCallback((vals: number[]) => {
    const val = vals[0] ?? 100;
    setLiveVolume(val);
  }, []);

  const handleVolumeChange = useCallback(
    (vals: number[]) => {
      const val = vals[0] ?? 100;
      setLiveVolume(val);
      executeAudioCommand(() => applyVolumeLevel(val, isConnectedRef.current) as Promise<DesktopAudioStatus>);
    },
    [executeAudioCommand]
  );

  // Mute toggle
  const handleToggleMute = useCallback(() => {
    const currentMuted = audioStateRef.current?.muted ?? false;
    executeAudioCommand(() => toggleAudioMuted(currentMuted, isConnectedRef.current) as Promise<DesktopAudioStatus>);
  }, [executeAudioCommand]);

  // Apply audio device
  const handleApplyDevice = useCallback(() => {
    executeAudioCommand(
      () => applyAudioDeviceSelection(selectedDeviceId, isConnectedRef.current, onReleaseInputs) as Promise<DesktopAudioStatus>,
      true
    );
  }, [selectedDeviceId, onReleaseInputs, executeAudioCommand]);

  // Refresh audio devices
  const handleRefreshDevices = useCallback(() => {
    executeAudioCommand(() => refreshAudioDeviceList(isConnectedRef.current) as Promise<DesktopAudioStatus>);
  }, [executeAudioCommand]);

  // Attach DOM listeners for legacy compatibility
  useEffect(() => {
    const el = document.getElementById("audio-volume");
    if (!el) return;
    const onInput = (e: any) => {
      const val = Number(e.target?.value ?? e?.detail?.value ?? (e as any).value);
      if (!Number.isNaN(val)) setLiveVolume(val);
    };
    const onChange = (e: any) => {
      const val = Number(e.target?.value ?? e?.detail?.value ?? (e as any).value);
      if (!Number.isNaN(val)) {
        setLiveVolume(val);
        executeAudioCommand(() => applyVolumeLevel(val, isConnectedRef.current) as Promise<DesktopAudioStatus>);
      }
    };
    el.addEventListener("input", onInput);
    el.addEventListener("change", onChange);
    return () => {
      el.removeEventListener("input", onInput);
      el.removeEventListener("change", onChange);
    };
  }, [executeAudioCommand]);

  useEffect(() => {
    const el = document.getElementById("session-quality");
    if (!el) return;
    const onChange = (e: any) => {
      const val = e.target?.value ?? e?.detail?.value ?? (e as any).value;
      if (val != null) {
        handleBitrateChange(String(val));
      }
    };
    el.addEventListener("change", onChange);
    return () => {
      el.removeEventListener("change", onChange);
    };
  }, [handleBitrateChange]);

  const disabledControls = !isConnected || audioPending || !audioState;
  const audioErrorText = audioError || audioState?.error || null;

  const audioStatusText = audioPending
    ? "Updating audio output"
    : audioState
      ? audioState.active
        ? "Output active"
        : "Output stopped"
      : "Output status unavailable";

  const devices: DesktopAudioDevice[] = audioState?.devices ?? [];

  return (
    <div className="flex flex-col gap-3">
      <Separator />

      {/* Video Quality Section */}
      <div className="text-[11px] font-medium text-muted-foreground">
        Quality
      </div>
      <div className="flex flex-col gap-1.5">
        <span id="session-quality-label" className="text-[12.5px] font-medium text-muted-foreground">
          Bitrate ceiling
        </span>
        <div
          id="session-quality"
          role="group"
          aria-labelledby="session-quality-label"
          aria-describedby="session-quality-error"
          className="grid h-9 grid-cols-4 gap-1 rounded-[10px] surface-well p-1"
        >
          {QUALITY_OPTIONS.map((mbps) => {
            const pressed = bitrate === mbps
            return (
              <button
                key={mbps}
                type="button"
                aria-pressed={pressed}
                onClick={() => handleBitrateChange(String(mbps))}
                className={`rounded-lg text-[12.5px] font-medium tabular-nums transition-colors duration-150 outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-ring ${
                  pressed ? "surface-raised text-foreground" : "text-muted-foreground hover:text-foreground"
                }`}
              >
                {mbps}
              </button>
            )
          })}
        </div>
        <span className="text-[11px] text-muted-foreground">Mbps</span>
      </div>
      <p
        id="session-quality-error"
        role="status"
        className="text-xs text-destructive"
        hidden={!qualityError}
      >
        {qualityError || ""}
      </p>

      <Separator />

      {/* Audio Output Section */}
      <div className="text-[11px] font-medium text-muted-foreground">
        Audio
      </div>
      <p id="audio-status" role="status" className="text-xs text-muted-foreground">
        {audioStatusText}
      </p>

      {/* Volume slider */}
      <div className="flex flex-col gap-1.5">
        <div className="flex justify-between items-center">
          <Label htmlFor="audio-volume" className="text-[12.5px] font-medium text-muted-foreground">
            Volume
          </Label>
          <span id="audio-volume-value" className="text-xs font-mono text-muted-foreground">
            {liveVolume}%
          </span>
        </div>
        <input
          id="audio-volume"
          type="range"
          min={0}
          max={100}
          step={1}
          value={liveVolume}
          onChange={(e) => handleVolumeInput([Number(e.target.value)])}
          onPointerUp={(e) => handleVolumeChange([Number((e.target as HTMLInputElement).value)])}
          onKeyUp={(e) => handleVolumeChange([Number((e.target as HTMLInputElement).value)])}
          disabled={disabledControls}
          aria-describedby="audio-error"
          className="h-1.5 w-full cursor-pointer appearance-none rounded-full bg-white/10 accent-[#ff6353] disabled:cursor-not-allowed disabled:opacity-50"
        />
      </div>

      {/* Output device selector */}
      <div className="flex flex-col gap-1.5">
        <span id="audio-device-label" className="text-[12.5px] font-medium text-muted-foreground">
          Output device
        </span>
        <div
          id="audio-device"
          role="group"
          aria-labelledby="audio-device-label"
          aria-describedby="audio-error"
          className="flex flex-col gap-1 rounded-[10px] surface-well p-1"
        >
          {[{ id: "default", name: "System default", supported: true }, ...devices].map((device) => {
            const pressed = selectedDeviceId === device.id
            return (
              <button
                key={device.id}
                type="button"
                aria-pressed={pressed}
                disabled={disabledControls || !device.supported}
                onClick={() => setSelectedDeviceId(device.id)}
                className={`h-8 truncate rounded-lg px-2.5 text-left text-[12.5px] font-medium transition-colors duration-150 outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-ring disabled:opacity-50 ${
                  pressed ? "surface-raised text-foreground" : "text-muted-foreground hover:text-foreground"
                }`}
              >
                {device.name}
              </button>
            )
          })}
        </div>
      </div>

      {/* Audio Action Buttons */}
      <div className="flex flex-wrap gap-2 pt-1">
        <Button
          type="button"
          variant="outline"
          size="sm"
          id="btn-audio-mute"
          aria-pressed={audioState?.muted ?? false}
          disabled={disabledControls}
          onClick={handleToggleMute}
        >
          {audioState?.muted ? "Unmute" : "Mute"}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          id="btn-audio-apply"
          disabled={disabledControls}
          aria-busy={audioPending}
          onClick={handleApplyDevice}
        >
          Use output
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          id="btn-audio-refresh"
          disabled={!isConnected || audioPending}
          onClick={handleRefreshDevices}
        >
          Refresh audio
        </Button>
      </div>

      <p
        id="audio-error"
        role="alert"
        className="text-xs text-destructive"
        hidden={!audioErrorText}
      >
        {audioErrorText || ""}
      </p>
    </div>
  );
}
