import { useCallback, useEffect, useMemo, useState } from "react"
import { ThisComputerCard } from "@/features/host/ThisComputerCard"
import { SavedCredentials } from "@/features/library/SavedCredentials"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  isNativeAvailable,
  listPairings,
  forgetPairing,
  getHostStatus,
  startHost,
  stopHost,
  type PairingSummary,
  type HostStatus,
} from "@/lib/ipc"
import type { ConnectionInstance } from "@/lib/connection"

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
  const [hostStatus, setHostStatus] = useState<HostStatus | null>(null)
  const [hostBusy, setHostBusy] = useState(false)
  const [pairings, setPairings] = useState<PairingSummary[]>([])
  const [connBusy, setConnBusy] = useState(() => connection.snapshot().busy)
  const [relayUrl, setRelayUrl] = useState(() => {
    if (typeof window === "undefined" || !window.localStorage) return ""
    return window.localStorage.getItem(RELAY_URL_STORAGE_KEY) ?? ""
  })

  useEffect(() => connection.subscribe((s) => setConnBusy(s.busy)), [connection])

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
    await connection.connect({
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
      </section>

      <SavedCredentials
        pairings={formattedPairings}
        busy={connBusy || !native}
        onConnect={handleConnectSaved}
        onForget={handleForget}
        onRefresh={refreshPairings}
      />

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
