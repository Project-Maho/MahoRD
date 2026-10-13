import { useState, useEffect, useMemo, useCallback, useRef } from "react"
import { CircleAlert, LoaderCircle, RefreshCw, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { cn } from "@/lib/utils"
import { LibraryToolbar } from "@/features/library/LibraryToolbar"
import { HostGrid } from "@/features/library/HostGrid"
import { DirectConnect } from "@/features/library/DirectConnect"
import {
  isNativeAvailable,
  invokeCommand,
  agentReleaseAll,
  forgetPairing,
  hideHost,
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

  const [removeTarget, setRemoveTarget] = useState<{
    id: string;
    name: string;
    ip: string;
  } | null>(null)
  const [removing, setRemoving] = useState(false)
  const [removeError, setRemoveError] = useState<string | null>(null)
  // Pairings deleted here but not yet re-read from list_hosts: their cards must
  // stop offering pairing-based connect until a refresh confirms the new truth.
  const [stalePairingIds, setStalePairingIds] = useState<string[]>([])
  const removeInFlight = useRef(false)

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
    const poll = () => {
      if (document.visibilityState !== "hidden") void library.refresh({ silent: true })
    }
    const timer = setInterval(poll, 4000)
    document.addEventListener("visibilitychange", poll)
    return () => {
      clearInterval(timer)
      document.removeEventListener("visibilitychange", poll)
    }
  }, [refreshHosts, library])

  const isPairedHost = useCallback(
    (host: { id: string; paired?: boolean | null }) =>
      Boolean(host.paired) && !stalePairingIds.includes(host.id),
    [stalePairingIds]
  )

  const focusRemovePanel = useCallback((node: HTMLDialogElement | null) => {
    node?.showModal?.()
  }, [])

  const handleRemoveRequest = (host: { id: string; ip: string; name: string }) => {
    if (removeTarget || removeInFlight.current) return
    setRemoveError(null)
    setRemoveTarget(host)
  }

  const handleRemoveCancel = () => {
    if (removeInFlight.current) return
    setRemoveTarget(null)
    setRemoveError(null)
  }

  const handleRemoveConfirm = async (forget = false) => {
    const target = removeTarget
    if (!native || !target || removeInFlight.current) return
    removeInFlight.current = true
    setRemoving(true)
    setRemoveError(null)
    try {
      if (forget && !stalePairingIds.includes(target.id)) {
        await forgetPairing(target.id)
        setStalePairingIds(prev =>
          prev.includes(target.id) ? prev : [...prev, target.id]
        )
      }
      await hideHost(target.name)
      setRemoveTarget(null)
      // refreshHosts swallows its own failures; status below tells us whether
      // the list is fresh again or still showing the pre-removal snapshot.
      await refreshHosts()
      if (library.snapshot().status === "ready") setStalePairingIds([])
    } catch (err) {
      const detail =
        err instanceof Error
          ? err.message
          : typeof err === "object" &&
              err !== null &&
              "message" in err &&
              typeof err.message === "string"
            ? err.message
            : String(err)
      setRemoveError(detail)
    } finally {
      removeInFlight.current = false
      setRemoving(false)
    }
  }

  const handleConnectHost = async (host: { id: string; ip: string; name: string }) => {
    if (!native) return
    const raw = libSnapshot.hosts.find((h) => h.id === host.id)
    if (raw && isPairedHost(raw)) {
      await connection.connect({
        host: host.ip,
        name: host.name,
        pairingId: host.id,
        tcpPort: raw.tcp_port ?? null,
        udpPort: raw.udp_port ?? null,
      })
    } else {
      if (connection.snapshot().busy) return
      await connection.connect({
        host: host.ip, name: host.name, pin: null, pairingId: null,
        tcpPort: raw?.tcp_port ?? null, udpPort: raw?.udp_port ?? null,
      })
      if (connection.snapshot().errorCode !== "pairing-required") return
      setSelectedPorts({
        tcpPort: raw?.tcp_port ?? null,
        udpPort: raw?.udp_port ?? null,
      })
      setDirectIp(host.ip)
      setDirectPin("")
      setDirectError("A PIN is needed for this new computer.")
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
      paired: isPairedHost(h),
    }))
  }, [displayHosts, isPairedHost])

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
          onRemove={handleRemoveRequest}
          removeBusy={removing}
          statusMessage={getStatusMessage()}
        />
      </div>

      {removeTarget && (
        <dialog
          ref={focusRemovePanel}
          aria-labelledby="remove-pairing-title"
          aria-describedby="remove-pairing-body"
          className="fixed inset-0 m-auto w-[min(420px,calc(100%-32px))] max-h-[calc(100%-32px)] overflow-y-auto bg-transparent p-0 text-foreground backdrop:bg-black/65 backdrop:backdrop-blur-md"
          onCancel={(event) => {
            event.preventDefault()
            handleRemoveCancel()
          }}
        >
          <Card
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="remove-pairing-title"
            aria-describedby="remove-pairing-body"
            data-testid="remove-pairing-confirm"
            tabIndex={-1}
            className="w-[min(420px,100%)] animate-panel-in outline-none"
          >
            <div className="flex items-start gap-3">
              <div className="grid size-9 shrink-0 place-items-center rounded-[9px] surface-raised">
                <Trash2 className="size-4 text-destructive" aria-hidden="true" />
              </div>
              <div className="min-w-0 flex-1 space-y-1">
                <h2
                  id="remove-pairing-title"
                  className="text-[15px] font-semibold leading-5 tracking-[-0.01em] text-foreground"
                >
                  Remove this computer?
                </h2>
                <p id="remove-pairing-body" className="text-sm text-muted-foreground">
                  Hide this computer from the list. You can restore it in Settings.
                </p>
              </div>
            </div>

            <div className="rounded-lg bg-white/[0.04] px-3 py-2 ring-1 ring-inset ring-white/[0.06]">
              <p
                className="truncate text-[13px] font-semibold text-foreground"
                title={removeTarget.name}
              >
                {removeTarget.name}
              </p>
              <p className="break-all select-text font-mono text-xs text-muted-foreground">
                {removeTarget.ip}
              </p>
            </div>

            {removeError && (
              <p
                id="remove-pairing-error"
                role="alert"
                className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/10 px-3 py-2 text-[13px] text-destructive"
              >
                <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
                <span>Could not remove this computer. {removeError}</span>
              </p>
            )}

            <div className="flex flex-wrap items-center justify-end gap-2">
              {libSnapshot.hosts.some(h => h.id === removeTarget.id && isPairedHost(h)) && (
                <Button type="button" variant="destructive" size="sm"
                  data-action="confirm-forget-paired" disabled={removing}
                  onClick={() => handleRemoveConfirm(true)}>
                  {removing ? "Removing" : "Remove and forget pairing"}
                </Button>
              )}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                data-action="cancel-remove-paired"
                disabled={removing}
                onClick={handleRemoveCancel}
              >
                Cancel
              </Button>
              <Button
                type="button"
                variant="destructive"
                size="sm"
                data-action="confirm-remove-paired"
                disabled={removing}
                aria-busy={removing}
                onClick={() => handleRemoveConfirm()}
                className="gap-1.5"
              >
                {removing ? (
                  <LoaderCircle
                    className="size-3.5 animate-spin motion-reduce:animate-none"
                    aria-hidden="true"
                  />
                ) : (
                  <Trash2 className="size-3.5" aria-hidden="true" />
                )}
                {removing ? "Removing" : "Remove from list"}
              </Button>
            </div>
          </Card>
        </dialog>
      )}

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
