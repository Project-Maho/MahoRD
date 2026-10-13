import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { CircleAlert } from "lucide-react"
import { ThisComputerCard } from "@/features/host/ThisComputerCard"
import { StartupSettings } from "@/features/settings/StartupSettings"
import { SavedCredentials } from "@/features/library/SavedCredentials"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Button } from "@/components/ui/button"
import {
  isNativeAvailable,
  listPairings,
  listHiddenHosts,
  unhideHost,
  forgetPairing,
  getHostStatus,
  startHost,
  stopHost,
  setAutoApprove,
  setHostPolicy,
  type PairingSummary,
  type HostStatus,
} from "@/lib/ipc"
import type { ConnectionInstance, ValidateConnectionOptions } from "@/lib/connection"
import { createLibrary } from "@/lib/library"
import { ConnectRecovery, connectionAttempts } from "@/features/library/ConnectRecovery"

export const RELAY_URL_STORAGE_KEY = "maho-relay-url"

export interface SettingsPageProps {
  connection: ConnectionInstance;
}

async function copyText(text: string): Promise<boolean> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // fall through to the execCommand path used by older webviews
    }
  }
  try {
    const ta = document.createElement("textarea")
    ta.value = text
    ta.style.position = "fixed"
    ta.style.opacity = "0"
    document.body.appendChild(ta)
    ta.focus()
    ta.select()
    const ok = document.execCommand("copy")
    document.body.removeChild(ta)
    return ok
  } catch {
    return false
  }
}

export function SettingsPage({ connection }: SettingsPageProps) {
  const native = isNativeAvailable()
  const [connSnapshot, setConnSnapshot] = useState(() => connection.snapshot())
  const lastRequest = useRef<ValidateConnectionOptions | null>(connectionAttempts.get(connection) || null)
  const [forgetTarget, setForgetTarget] = useState<string | null>(null)
  const [connectActionError, setConnectActionError] = useState<string | null>(null)
  const connectSavedRequest = async (request: ValidateConnectionOptions) => {
    if (connection.snapshot().busy) return
    lastRequest.current = { ...request }
    connectionAttempts.set(connection, { ...request })
    const pending = connection.connect(request)
    const generation = connection.snapshot().generation
    await pending
    const state = connection.snapshot()
    if (state.generation === generation && !state.error && (state.phase === "waiting-video" || state.phase === "streaming")) {
      const library = createLibrary({ nativeAvailable: native, invoke: async () => [], storage: typeof window === "undefined" ? null : window.localStorage })
      library.recordConnection({ id: request.pairingId || request.host || "", name: request.name, ip: request.host || "", online: true, paired: true, tcp_port: request.tcpPort, udp_port: request.udpPort })
    }
  }
  const [hostStatus, setHostStatus] = useState<HostStatus | null>(null)
  const [hostBusy, setHostBusy] = useState(false)
  const [pairings, setPairings] = useState<PairingSummary[]>([])
  const [hiddenHosts, setHiddenHosts] = useState<string[]>([])
  const [hiddenBusy, setHiddenBusy] = useState(false)
  const [hiddenError, setHiddenError] = useState<string | null>(null)
  useEffect(() => {
    if (!native) return
    let active = true
    listHiddenHosts().then(names => { if (active) setHiddenHosts(names) })
      .catch(err => { if (active) setHiddenError(String(err?.message ?? err)) })
    return () => { active = false }
  }, [native])
  const restoreHost = async (name: string) => {
    if (hiddenBusy) return
    setHiddenBusy(true)
    setHiddenError(null)
    try {
      await unhideHost(name)
      setHiddenHosts(names => names.filter(item => item !== name))
    } catch (err) {
      setHiddenError(err instanceof Error ? err.message : String(err))
    } finally {
      setHiddenBusy(false)
    }
  }
  const [connBusy, setConnBusy] = useState(() => connection.snapshot().busy)
  const [relayUrl, setRelayUrl] = useState(() => {
    if (typeof window === "undefined" || !window.localStorage) return ""
    return window.localStorage.getItem(RELAY_URL_STORAGE_KEY) ?? ""
  })
  const [autoApproveBusy, setAutoApproveBusy] = useState(false)
  const [autoApproveError, setAutoApproveError] = useState<string | null>(null)
  const [policyBusy, setPolicyBusy] = useState(false)
  const [policyError, setPolicyError] = useState<string | null>(null)

  useEffect(() => connection.subscribe((s) => { setConnBusy(s.busy); setConnSnapshot(s) }), [connection])

  useEffect(() => {
    if (typeof window === "undefined" || !window.localStorage) return
    window.localStorage.setItem(RELAY_URL_STORAGE_KEY, relayUrl)
  }, [relayUrl])

  const refreshPairings = useCallback(async () => {
    if (!native) return
    try {
      const list = await listPairings()
      if (Array.isArray(list)) setPairings(list)
    } catch (err) {
      console.error("Failed to refresh pairings:", err)
    }
  }, [native])

  useEffect(() => {
    if (!native) return
    getHostStatus()
      .then((s) => s && setHostStatus(s))
      .catch((err) => console.error("Failed to load host status:", err))
    refreshPairings()
  }, [native, refreshPairings])

  const handleToggleSharing = async () => {
    if (!native) return
    setHostBusy(true)
    try {
      const isRunning = hostStatus ? hostStatus.running : true
      const next = isRunning ? await stopHost() : await startHost()
      if (next) setHostStatus(next)
    } catch (err) {
      console.error("Failed to toggle sharing:", err)
    } finally {
      setHostBusy(false)
    }
  }

  const handleToggleAutoApprove = async () => {
    if (!native || autoApproveBusy || hostBusy) return
    const next = !(hostStatus?.auto_approve ?? false)
    setAutoApproveBusy(true)
    setAutoApproveError(null)
    try {
      await setAutoApprove(next)
      const updated = await getHostStatus()
      if (updated) setHostStatus(updated)
    } catch (err: any) {
      console.error("Failed to toggle auto-approve:", err)
      setAutoApproveError(String(err?.message ?? err ?? "Failed to toggle auto-approve"))
    } finally {
      setAutoApproveBusy(false)
    }
  }

  const handleToggleAllowInput = async () => {
    if (!native || policyBusy || hostBusy) return
    const currentInput = hostStatus?.allow_input ?? true
    const currentClipboard = hostStatus?.allow_clipboard ?? true
    const nextInput = !currentInput
    setPolicyBusy(true)
    setPolicyError(null)
    try {
      const updated = await setHostPolicy({
        allowInput: nextInput,
        allowClipboard: currentClipboard,
      })
      if (updated) {
        setHostStatus(updated)
      } else {
        const refreshed = await getHostStatus()
        if (refreshed) setHostStatus(refreshed)
      }
    } catch (err: any) {
      console.error("Failed to update host policy:", err)
      setPolicyError(String(err?.message ?? err ?? "Failed to update host policy"))
    } finally {
      setPolicyBusy(false)
    }
  }

  const handleToggleAllowClipboard = async () => {
    if (!native || policyBusy || hostBusy) return
    const currentInput = hostStatus?.allow_input ?? true
    const currentClipboard = hostStatus?.allow_clipboard ?? true
    const nextClipboard = !currentClipboard
    setPolicyBusy(true)
    setPolicyError(null)
    try {
      const updated = await setHostPolicy({
        allowInput: currentInput,
        allowClipboard: nextClipboard,
      })
      if (updated) {
        setHostStatus(updated)
      } else {
        const refreshed = await getHostStatus()
        if (refreshed) setHostStatus(refreshed)
      }
    } catch (err: any) {
      console.error("Failed to update host policy:", err)
      setPolicyError(String(err?.message ?? err ?? "Failed to update host policy"))
    } finally {
      setPolicyBusy(false)
    }
  }

  const handleCopyInfo = () => {
    const ip = hostStatus?.ip || "Unavailable"
    const port = hostStatus?.port || 19730
    const pin = hostStatus?.pin || "--------"
    return copyText(`${ip}:${port} PIN: ${pin}`)
  }

  const handleConnectSaved = async (id: string) => {
    if (!native) return
    const pairing = pairings.find((p) => p.id === id)
    if (!pairing?.lastEndpoint?.host) return
    await connectSavedRequest({
      host: pairing.lastEndpoint.host,
      name: pairing.hostName,
      pairingId: pairing.id,
      tcpPort: pairing.lastEndpoint.tcpPort,
      udpPort: pairing.lastEndpoint.udpPort,
    })
  }

  const handleForget = async (id: string) => {
    if (!native) return
    try {
      await forgetPairing(id)
      await refreshPairings()
    } catch (err) {
      console.error("Failed to forget pairing:", err)
    }
  }

  const formattedPairings = useMemo(
    () =>
      pairings.map((p) => ({
        id: p.id,
        name: p.hostName,
        endpoint: p.lastEndpoint
          ? `${p.lastEndpoint.host}:${p.lastEndpoint.tcpPort} (UDP ${p.lastEndpoint.udpPort})`
          : null,
      })),
    [pairings]
  )

  return (
    <div className="flex flex-col gap-8">
      <h1 className="text-[26px] font-semibold leading-8 tracking-[-0.02em] text-foreground">
        Settings
      </h1>

      <StartupSettings />

      <section className="space-y-3" aria-label="Sharing">
        <h2 className="text-[13px] font-medium text-muted-foreground">Sharing</h2>
        <ThisComputerCard
          ip={hostStatus?.ip ?? null}
          pin={hostStatus?.pin ?? null}
          running={hostStatus?.running ?? false}
          busy={hostBusy || !native}
          onToggleSharing={handleToggleSharing}
          onCopyInfo={handleCopyInfo}
        />

        <div className="rounded-panel surface-card p-4 space-y-3">
          <div className="flex items-center justify-between gap-4">
            <div className="space-y-0.5">
              <label
                htmlFor="toggle-auto-approve"
                className="text-[13px] font-medium text-foreground cursor-pointer"
              >
                Auto-approve new devices
              </label>
              <p className="text-xs text-subtle-foreground">
                Skip the approval prompt when a device pairs with this computer&apos;s PIN.
              </p>
            </div>
            <button
              id="toggle-auto-approve"
              type="button"
              role="switch"
              aria-checked={hostStatus?.auto_approve ?? false}
              disabled={!native || hostBusy || autoApproveBusy}
              onClick={handleToggleAutoApprove}
              className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors duration-150 outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50 ${
                hostStatus?.auto_approve ? "bg-primary" : "bg-white/10"
              }`}
            >
              <span
                className={`pointer-events-none inline-block size-4 transform rounded-full bg-white shadow-sm transition-transform duration-150 ${
                  hostStatus?.auto_approve ? "translate-x-4" : "translate-x-0.5"
                }`}
              />
            </button>
          </div>

          {autoApproveError && (
            <div
              role="alert"
              className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/10 px-3 py-2 text-[13px] text-destructive"
            >
              <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              <span>{autoApproveError}</span>
            </div>
          )}

          <div className="border-t border-white/5 pt-3">
            <div className="flex items-center justify-between gap-4">
              <div className="space-y-0.5">
                <label
                  htmlFor="toggle-allow-input"
                  className="text-[13px] font-medium text-foreground cursor-pointer"
                >
                  Allow remote keyboard &amp; mouse
                </label>
                <p className="text-xs text-subtle-foreground">
                  Let connected clients control this computer's keyboard and mouse.
                </p>
              </div>
              <button
                id="toggle-allow-input"
                type="button"
                role="switch"
                aria-checked={hostStatus?.allow_input ?? true}
                disabled={!native || hostBusy || policyBusy}
                onClick={handleToggleAllowInput}
                className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors duration-150 outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50 ${
                  (hostStatus?.allow_input ?? true) ? "bg-primary" : "bg-white/10"
                }`}
              >
                <span
                  className={`pointer-events-none inline-block size-4 transform rounded-full bg-white shadow-sm transition-transform duration-150 ${
                    (hostStatus?.allow_input ?? true) ? "translate-x-4" : "translate-x-0.5"
                  }`}
                />
              </button>
            </div>
          </div>

          <div className="border-t border-white/5 pt-3">
            <div className="flex items-center justify-between gap-4">
              <div className="space-y-0.5">
                <label
                  htmlFor="toggle-allow-clipboard"
                  className="text-[13px] font-medium text-foreground cursor-pointer"
                >
                  Allow clipboard sync
                </label>
                <p className="text-xs text-subtle-foreground">
                  Synchronize text clipboard content between this host and connected clients.
                </p>
              </div>
              <button
                id="toggle-allow-clipboard"
                type="button"
                role="switch"
                aria-checked={hostStatus?.allow_clipboard ?? true}
                disabled={!native || hostBusy || policyBusy}
                onClick={handleToggleAllowClipboard}
                className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors duration-150 outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50 ${
                  (hostStatus?.allow_clipboard ?? true) ? "bg-primary" : "bg-white/10"
                }`}
              >
                <span
                  className={`pointer-events-none inline-block size-4 transform rounded-full bg-white shadow-sm transition-transform duration-150 ${
                    (hostStatus?.allow_clipboard ?? true) ? "translate-x-4" : "translate-x-0.5"
                  }`}
                />
              </button>
            </div>
          </div>

          {policyError && (
            <div
              role="alert"
              className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/10 px-3 py-2 text-[13px] text-destructive"
            >
              <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              <span>{policyError}</span>
            </div>
          )}
        </div>
      </section>

      <SavedCredentials
        pairings={formattedPairings}
        busy={connBusy || !native}
        onConnect={handleConnectSaved}
        onForget={handleForget}
        onRefresh={refreshPairings}
      />
      <ConnectRecovery error={connSnapshot.error} code={connSnapshot.errorCode} paired busy={connBusy}
        onRetry={() => { if (lastRequest.current) void connectSavedRequest(lastRequest.current) }}
        onCleanup={() => void connection.retryCleanup()}
        onForget={() => setForgetTarget(lastRequest.current?.pairingId || null)} />
      {forgetTarget && <section role="alertdialog" aria-label="Confirm forgetting saved pairing" className="space-y-3 rounded-panel surface-card p-4">
        <p className="text-sm">Forget this saved pairing? Your own computer will need authorization again. This does not repair network failures.</p>
        {connectActionError && <p role="alert" className="text-sm text-destructive">{connectActionError}</p>}
        <Button size="sm" variant="destructive" disabled={connBusy} onClick={async () => {
          try { await forgetPairing(forgetTarget); setForgetTarget(null); setConnectActionError(null); await refreshPairings() }
          catch (err) { setConnectActionError(err instanceof Error ? err.message : String(err)) }
        }}>Confirm forget pairing</Button>
        <Button size="sm" variant="ghost" onClick={() => setForgetTarget(null)}>Cancel</Button>
      </section>}

      <section id="hidden-computers-section" className="space-y-3" aria-labelledby="hidden-computers-heading">
        <h2 id="hidden-computers-heading" className="text-[15px] font-semibold text-foreground">Hidden computers <span className="text-muted-foreground">{hiddenHosts.length}</span></h2>
        {hiddenError && <p role="alert" className="text-sm text-destructive">{hiddenError}</p>}
        {!hiddenHosts.length ? <p className="rounded-panel border border-dashed border-white/10 px-4 py-4 text-sm text-muted-foreground">No hidden computers.</p> : (
          <div id="hidden-computers-list" role="list" className="divide-y divide-white/[0.06] overflow-hidden rounded-panel surface-card">
            {hiddenHosts.map(name => <div key={name} role="listitem" className="flex items-center gap-3 px-4 py-3">
              <span className="min-w-0 flex-1 break-all text-sm font-semibold">{name}</span>
              <Button size="sm" variant="ghost" data-action="restore-host" disabled={hiddenBusy || !native} onClick={() => restoreHost(name)}>Restore</Button>
            </div>)}
          </div>
        )}
      </section>

      <section className="space-y-3" aria-labelledby="relay-heading">
        <h2 id="relay-heading" className="text-[13px] font-medium text-muted-foreground">
          Relay
        </h2>
        <div className="space-y-2 rounded-panel surface-card p-4">
          <Label htmlFor="direct-relay" className="text-[13px] font-medium text-foreground">
            Relay URL
          </Label>
          <Input
            id="direct-relay"
            type="text"
            value={relayUrl}
            onChange={(e) => setRelayUrl(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            placeholder="wss://relay.example"
            aria-describedby="relay-help"
            className="font-mono"
          />
          <p id="relay-help" className="text-xs text-subtle-foreground">
            Used as a fallback when a direct connection fails. Leave empty to connect directly only.
          </p>
        </div>
      </section>
    </div>
  )
}

export default SettingsPage
