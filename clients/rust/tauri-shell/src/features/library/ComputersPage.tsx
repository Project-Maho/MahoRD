import { useState, useEffect, useMemo, useCallback } from "react"
import { Button } from "@/components/ui/button"
import { ThisComputerCard } from "@/features/host/ThisComputerCard"
import { LibraryToolbar, type FilterOption } from "@/features/library/LibraryToolbar"
import { SavedCredentials } from "@/features/library/SavedCredentials"
import { HostGrid } from "@/features/library/HostGrid"
import { DirectConnect } from "@/features/library/DirectConnect"
import {
  isNativeAvailable,
  invokeCommand,
  listPairings,
  forgetPairing,
  getHostStatus,
  startHost,
  stopHost,
  agentReleaseAll,
  type HostItem,
  type PairingSummary,
  type HostStatus,
  type MahoCommand,
} from "@/lib/ipc"
import {
  createLibrary,
  normalizeHostKey,
  type Library,
  type LibrarySnapshot,
} from "@/lib/library"
import {
  createConnection,
  type ConnectionInstance,
  type ConnectionSnapshot,
} from "@/lib/connection"

export interface LibraryHost extends HostItem {
  [key: string]: unknown;
}

export interface ComputersPageProps {
  view?: "computers" | "favorites";
  activeView?: "computers" | "favorites";
  includeDirectConnect?: boolean;
  connection?: ConnectionInstance;
}

export function ComputersPage({
  view,
  activeView,
  includeDirectConnect,
  connection: propConnection,
}: ComputersPageProps) {
  const currentView = view ?? activeView ?? "computers"
  const isFavoritesView = currentView === "favorites"
  const showDirect =
    includeDirectConnect !== undefined
      ? includeDirectConnect
      : !isFavoritesView

  const native = isNativeAvailable()

  // Injected dependency IPC seam preserved
  const library: Library<LibraryHost> = useMemo(() => {
    return createLibrary<LibraryHost>({
      invoke: (cmd: string, ...args: unknown[]) =>
        invokeCommand(cmd as MahoCommand, args[0] as Record<string, unknown> | undefined),
      storage: typeof window !== "undefined" ? window.localStorage : null,
      nativeAvailable: native,
    })
  }, [native])

  const connection: ConnectionInstance = useMemo(() => {
    return (
      propConnection ??
      createConnection({
        invoke: (cmd: string, args?: unknown) =>
          invokeCommand(cmd as MahoCommand, args as Record<string, unknown> | undefined),
        nativeAvailable: native,
        releaseInputs: async () => {
          try {
            await agentReleaseAll()
          } catch {
            // Release failure is non-fatal during teardown
          }
        },
      })
    )
  }, [propConnection, native])

  const [libSnapshot, setLibSnapshot] = useState<LibrarySnapshot<LibraryHost>>(() =>
    library.snapshot()
  )
  const [connSnapshot, setConnSnapshot] = useState<ConnectionSnapshot>(() =>
    connection.snapshot()
  )

  const [hostStatus, setHostStatus] = useState<HostStatus | null>(null)
  const [pairings, setPairings] = useState<PairingSummary[]>([])
  const [hostBusy, setHostBusy] = useState(false)
  const [refreshing, setRefreshing] = useState(false)

  const [directIp, setDirectIp] = useState("")
  const [relayUrl, setRelayUrl] = useState(() => {
    if (typeof window === "undefined" || !window.localStorage) return ""
    return window.localStorage.getItem("maho-relay-url") ?? ""
  })
  useEffect(() => {
    if (typeof window === "undefined" || !window.localStorage) return
    window.localStorage.setItem("maho-relay-url", relayUrl)
  }, [relayUrl])
  const [directPin, setDirectPin] = useState("")
  const [directError, setDirectError] = useState<string | null>(null)
  const [selectedPairingId, setSelectedPairingId] = useState<string | null>(null)
  const [selectedPorts, setSelectedPorts] = useState<{
    tcpPort?: number | null;
    udpPort?: number | null;
  }>({})

  useEffect(() => {
    const unsub = library.subscribe(setLibSnapshot)
    return () => {
      unsub()
    }
  }, [library])

  useEffect(() => {
    return connection.subscribe(setConnSnapshot)
  }, [connection])

  // Sync view selection with library filters
  useEffect(() => {
    if (isFavoritesView) {
      library.setFavoritesOnly(true)
    } else {
      library.clearFilters()
    }
  }, [isFavoritesView, library])

  // Initial data loading on mount when running inside Tauri
  const loadInitialData = useCallback(async () => {
    if (!native) return
    setRefreshing(true)
    try {
      const [statusResult, pairingsResult] = await Promise.allSettled([
        getHostStatus(),
        listPairings(),
        library.refresh(),
      ])
      if (statusResult.status === "fulfilled" && statusResult.value) {
        setHostStatus(statusResult.value)
      }
      if (pairingsResult.status === "fulfilled" && Array.isArray(pairingsResult.value)) {
        setPairings(pairingsResult.value)
      }
    } catch (err) {
      console.error("Failed to load initial data:", err)
    } finally {
      setRefreshing(false)
    }
  }, [native, library])

  useEffect(() => {
    loadInitialData()
  }, [loadInitialData])

  const handleRefreshAll = async () => {
    if (!native) return
    setRefreshing(true)
    try {
      const [statusResult, pairingsResult] = await Promise.allSettled([
        getHostStatus(),
        listPairings(),
        library.refresh(),
      ])
      if (statusResult.status === "fulfilled" && statusResult.value) {
        setHostStatus(statusResult.value)
      }
      if (pairingsResult.status === "fulfilled" && Array.isArray(pairingsResult.value)) {
        setPairings(pairingsResult.value)
      }
    } catch (err) {
      console.error("Failed to refresh:", err)
    } finally {
      setRefreshing(false)
    }
  }

  const handleRefreshPairings = async () => {
    if (!native) return
    try {
      const list = await listPairings()
      if (Array.isArray(list)) {
        setPairings(list)
      }
    } catch (err) {
      console.error("Failed to refresh pairings:", err)
    }
  }

  const handleForgetPairing = async (id: string) => {
    if (!native) return
    try {
      await forgetPairing(id)
      await handleRefreshPairings()
    } catch (err) {
      console.error("Failed to forget pairing:", err)
    }
  }

  const handleToggleSharing = async () => {
    if (!native) return
    setHostBusy(true)
    try {
      const isRunning = hostStatus ? hostStatus.running : true
      const nextStatus = isRunning ? await stopHost() : await startHost()
      if (nextStatus) {
        setHostStatus(nextStatus)
      }
    } catch (err) {
      console.error("Failed to toggle sharing:", err)
    } finally {
      setHostBusy(false)
    }
  }

  const handleCopyInfo = async () => {
    const ip = hostStatus?.ip || "Unavailable"
    const port = hostStatus?.port || 19730
    const pin = hostStatus?.pin || "--------"
    const info = `${ip}:${port} PIN: ${pin}`

    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      try {
        await navigator.clipboard.writeText(info)
        return
      } catch {
        // Fallback below
      }
    }
    try {
      const ta = document.createElement("textarea")
      ta.value = info
      ta.style.position = "fixed"
      ta.style.opacity = "0"
      document.body.appendChild(ta)
      ta.focus()
      ta.select()
      document.execCommand("copy")
      document.body.removeChild(ta)
    } catch {
      // Ignore clipboard fallback failure
    }
  }

  const handleConnectHost = async (host: { id: string; ip: string; name: string }) => {
    if (!native) return
    const raw = libSnapshot.hosts.find((h) => h.id === host.id)
    if (raw?.paired) {
      await connection.connect({
        host: host.ip,
        name: host.name,
        pairingId: host.id,
        tcpPort: raw.tcp_port ?? null,
        udpPort: raw.udp_port ?? null,
      })
    } else {
      setSelectedPairingId(null)
      setSelectedPorts({
        tcpPort: raw?.tcp_port ?? null,
        udpPort: raw?.udp_port ?? null,
      })
      setDirectIp(host.ip)
      setDirectPin("")
      setDirectError(null)
      const pinEl = document.getElementById("direct-pin")
      pinEl?.focus()
      pinEl?.scrollIntoView?.({ behavior: "smooth", block: "center" })
    }
  }

  const handleConnectSaved = async (id: string) => {
    if (!native) return
    const pairing = pairings.find((p) => p.id === id)
    if (!pairing) return
    if (pairing.lastEndpoint?.host) {
      await connection.connect({
        host: pairing.lastEndpoint.host,
        name: pairing.hostName,
        pairingId: pairing.id,
        tcpPort: pairing.lastEndpoint.tcpPort,
        udpPort: pairing.lastEndpoint.udpPort,
      })
    } else {
      setSelectedPairingId(pairing.id)
      setSelectedPorts({})
      setDirectIp("")
      setDirectPin("")
      setDirectError(null)
      const ipEl = document.getElementById("direct-ip")
      ipEl?.focus()
    }
  }

  const handleDirectConnect = async () => {
    if (!native) {
      setDirectError("Desktop connection controls are unavailable in this browser.")
      return
    }
    const host = directIp.trim()
    if (!host) {
      setDirectError("Enter a host address.")
      return
    }
    const pin = directPin.trim() || null
    if (pin && !/^[0-9]{8}$/.test(pin)) {
      setDirectError("Use exactly eight ASCII digits, or leave PIN blank for saved pairing.")
      return
    }
    setDirectError(null)
    const pairingId = pin ? null : selectedPairingId
    const ports = selectedPorts.tcpPort ? selectedPorts : {}

    const ok = await connection.connect({
      host,
      name: host,
      pin,
      tcpPort: ports.tcpPort ?? null,
      udpPort: ports.udpPort ?? null,
      pairingId,
    })

    if (!ok) {
      const snap = connection.snapshot()
      if (snap.fieldErrors.host) {
        setDirectError("Enter a host address.")
      } else if (snap.fieldErrors.pin) {
        setDirectError("Use exactly eight ASCII digits, or leave PIN blank for saved pairing.")
      } else if (snap.error) {
        setDirectError(snap.error)
      }
    }
  }

  const currentToolbarFilter: FilterOption = libSnapshot.favoritesOnly
    ? "favorites"
    : libSnapshot.availableOnly
      ? "available"
      : "all"

  const handleToolbarFilterChange = (f: FilterOption) => {
    if (f === "all") {
      library.clearFilters()
    } else if (f === "available") {
      library.clearFilters()
      library.setAvailableOnly(true)
    } else if (f === "favorites") {
      library.clearFilters()
      library.setFavoritesOnly(true)
    }
  }

  const getStatusMessage = () => {
    const list = isFavoritesView ? favoriteHosts : libSnapshot.visibleHosts
    if (list.length > 0) return null
    if (libSnapshot.status === "unavailable") {
      return "Desktop connection controls are unavailable in this browser."
    }
    if (libSnapshot.status === "error") {
      return `Computers not refreshed: ${libSnapshot.error}`
    }
    if (libSnapshot.status === "loading") {
      return "Loading computers"
    }
    if (!libSnapshot.hosts.length) {
      return "No computers listed. Connect by address below."
    }
    if (isFavoritesView || libSnapshot.favoritesOnly) {
      return "No favorites here. Star a computer to keep it in this local view."
    }
    if (libSnapshot.query.trim()) {
      return "No matching computers."
    }
    return "No available computers match these filters."
  }

  const normalizedFavorites = useMemo(
    () => new Set(libSnapshot.favoriteIps.map(normalizeHostKey)),
    [libSnapshot.favoriteIps]
  )

  const favoriteHosts = useMemo(
    () => libSnapshot.hosts.filter((h) => normalizedFavorites.has(normalizeHostKey(h.ip))),
    [libSnapshot.hosts, normalizedFavorites]
  )

  const displayHosts = isFavoritesView ? favoriteHosts : libSnapshot.visibleHosts

  const formattedHosts = useMemo(() => {
    return displayHosts.map((h) => ({
      id: h.id,
      name: h.name || h.ip,
      ip: h.ip,
      os: h.os ?? null,
      online: typeof h.online === "boolean" ? h.online : Boolean(h.online),
      paired: Boolean(h.paired),
    }))
  }, [displayHosts])

  const formattedPairings = useMemo(() => {
    return pairings.map((p) => ({
      id: p.id,
      name: p.hostName,
      endpoint: p.lastEndpoint
        ? `${p.lastEndpoint.host}:${p.lastEndpoint.tcpPort} (UDP ${p.lastEndpoint.udpPort})`
        : null,
    }))
  }, [pairings])

  const directCombinedError =
    directError ||
    connSnapshot.error ||
    (connSnapshot.fieldErrors.host
      ? "Enter a host address."
      : connSnapshot.fieldErrors.pin
        ? "Use exactly eight ASCII digits, or leave PIN blank for saved pairing."
        : null)

  return (
    <div className="space-y-6">
      <header className="dashboard-header flex items-center justify-between gap-4 max-[720px]:flex-wrap max-[720px]:pb-4">
        <div className="dash-titles space-y-1">
          <h1 className="text-2xl font-bold tracking-tight text-foreground">
            {isFavoritesView ? "Favorites" : "Computers"}
          </h1>
          <p className="text-sm text-muted-foreground">
            {isFavoritesView
              ? "Starred computers for quick access."
              : "Find a computer or connect by address."}
          </p>
        </div>
        <Button
          id="btn-refresh"
          variant="outline"
          size="sm"
          onClick={handleRefreshAll}
          disabled={refreshing || libSnapshot.status === "loading" || !native}
          aria-busy={refreshing || libSnapshot.status === "loading"}
        >
          Refresh
        </Button>
      </header>

      {!isFavoritesView && (
        <ThisComputerCard
          ip={hostStatus?.ip ?? null}
          pin={hostStatus?.pin ?? null}
          running={hostStatus?.running ?? false}
          busy={hostBusy || !native}
          onToggleSharing={handleToggleSharing}
          onCopyInfo={handleCopyInfo}
        />
      )}

      {!isFavoritesView && (
        <LibraryToolbar
          query={libSnapshot.query}
          onQueryChange={(q) => library.setQuery(q)}
          onClearSearch={() => library.setQuery("")}
          filter={currentToolbarFilter}
          onFilterChange={handleToolbarFilterChange}
        />
      )}

      {!isFavoritesView && (
        <SavedCredentials
          pairings={formattedPairings}
          busy={connSnapshot.busy || refreshing || !native}
          onConnect={handleConnectSaved}
          onForget={handleForgetPairing}
          onRefresh={handleRefreshPairings}
        />
      )}

      <HostGrid
        hosts={formattedHosts}
        favoriteIps={libSnapshot.favoriteIps}
        busy={connSnapshot.busy || !native}
        onConnect={handleConnectHost}
        onToggleFavorite={(ip) => library.toggleFavorite(ip)}
        statusMessage={getStatusMessage()}
      />

      {libSnapshot.preferenceError && (
        <div id="preference-error" role="status" className="text-xs text-warning">
          {libSnapshot.preferenceError}
        </div>
      )}

      {showDirect && (
        <DirectConnect
          ip={directIp}
          pin={directPin}
          relayUrl={relayUrl}
          onRelayUrlChange={setRelayUrl}
          onIpChange={(v) => {
            setDirectIp(v)
            setDirectError(null)
          }}
          onPinChange={(v) => {
            setDirectPin(v)
            setDirectError(null)
          }}
          onConnect={handleDirectConnect}
          busy={connSnapshot.busy}
          error={directCombinedError}
        />
      )}
    </div>
  )
}

export default ComputersPage
