import { useState, useEffect, useMemo, useCallback } from "react"
import { RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import { LibraryToolbar } from "@/features/library/LibraryToolbar"
import { HostGrid } from "@/features/library/HostGrid"
import { DirectConnect } from "@/features/library/DirectConnect"
import {
  isNativeAvailable,
  invokeCommand,
  agentReleaseAll,
  type HostItem,
  type MahoCommand,
} from "@/lib/ipc"
import {
  createLibrary,
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
  includeDirectConnect?: boolean;
  connection?: ConnectionInstance;
}

export function ComputersPage({
  includeDirectConnect = true,
  connection: propConnection,
}: ComputersPageProps) {
  const showDirect = includeDirectConnect

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

  const [refreshing, setRefreshing] = useState(false)

  const [directIp, setDirectIp] = useState("")
  const [directPin, setDirectPin] = useState("")
  const [directError, setDirectError] = useState<string | null>(null)
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

  useEffect(() => {
    library.clearFilters()
  }, [library])

  const refreshHosts = useCallback(async () => {
    if (!native) return
    setRefreshing(true)
    try {
      await library.refresh()
    } catch (err) {
      console.error("Failed to refresh computers:", err)
    } finally {
      setRefreshing(false)
    }
  }, [native, library])

  useEffect(() => {
    refreshHosts()
  }, [refreshHosts])

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
    const ports = selectedPorts.tcpPort ? selectedPorts : {}

    const ok = await connection.connect({
      host,
      name: host,
      pin,
      tcpPort: ports.tcpPort ?? null,
      udpPort: ports.udpPort ?? null,
      pairingId: null,
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

  const getStatusMessage = () => {
    if (libSnapshot.visibleHosts.length > 0) return null
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
      return "No computers found yet. Refresh, or connect by address below."
    }
    return "No matching computers."
  }

  const displayHosts = libSnapshot.visibleHosts

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

  const directCombinedError =
    directError ||
    connSnapshot.error ||
    (connSnapshot.fieldErrors.host
      ? "Enter a host address."
      : connSnapshot.fieldErrors.pin
        ? "Use exactly eight ASCII digits, or leave PIN blank for saved pairing."
        : null)

  const isRefreshing = refreshing || libSnapshot.status === "loading"

  return (
    <div className="flex flex-col gap-8">
      <header className="dashboard-header flex items-center justify-between gap-4">
        <h1 className="text-[26px] font-semibold leading-8 tracking-[-0.02em] text-foreground">
          Computers
        </h1>
        <Button
          id="btn-refresh"
          variant="ghost"
          size="icon-sm"
          onClick={refreshHosts}
          disabled={isRefreshing || !native}
          aria-busy={isRefreshing}
          aria-label="Refresh computers"
          title="Refresh computers"
        >
          <RefreshCw
            className={cn(isRefreshing && native && "animate-spin motion-reduce:animate-none")}
            aria-hidden="true"
          />
        </Button>
      </header>

      <div className="flex flex-col gap-4">
        {libSnapshot.hosts.length > 0 ? (
          <LibraryToolbar
            query={libSnapshot.query}
            onQueryChange={(q) => library.setQuery(q)}
            onClearSearch={() => library.setQuery("")}
          />
        ) : null}

        <HostGrid
          hosts={formattedHosts}
          busy={connSnapshot.busy || !native}
          onConnect={handleConnectHost}
          statusMessage={getStatusMessage()}
        />
      </div>

      {showDirect && (
        <DirectConnect
          ip={directIp}
          pin={directPin}
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
