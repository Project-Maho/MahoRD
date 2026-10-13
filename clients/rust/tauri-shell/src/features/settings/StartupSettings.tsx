import { useEffect, useState } from "react"

interface NativeBridge {
  readonly core?: {
    readonly invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>
  }
}

declare global {
  interface Window {
    readonly __TAURI__?: NativeBridge
  }
}

function bridge(): NativeBridge | undefined {
  return window.__TAURI__
}

export function StartupSettings() {
  const [enabled, setEnabled] = useState(false)
  const [busy, setBusy] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const native = bridge()?.core

  useEffect(() => {
    let mounted = true
    if (!native) {
      setBusy(false)
      return
    }
    native.invoke<boolean>("get_autostart").then(
      (value) => { if (mounted) { setEnabled(value); setBusy(false) } },
      (reason: unknown) => {
        if (mounted) {
          setError(reason instanceof Error ? reason.message : String(reason))
          setBusy(false)
        }
      },
    )
    return () => { mounted = false }
  }, [native])

  async function toggle() {
    if (!native || busy) return
    setBusy(true)
    setError(null)
    try {
      setEnabled(await native.invoke<boolean>("set_autostart", { enabled: !enabled }))
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="space-y-3" aria-label="Startup">
      <h2 className="text-[13px] font-medium text-muted-foreground">Startup</h2>
      <div className="rounded-panel surface-card p-4 space-y-3">
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5">
            <label htmlFor="toggle-autostart" className="text-[13px] font-medium text-foreground cursor-pointer">
              Start MahoRD at login
            </label>
            <p id="autostart-description" className="text-xs text-subtle-foreground">
              Keep hosting available after you sign in. Closing the window while hosting keeps MahoRD in the tray.
            </p>
          </div>
          <button
            id="toggle-autostart"
            type="button"
            role="switch"
            aria-checked={enabled}
            aria-describedby="autostart-description"
            aria-busy={busy}
            disabled={!native || busy}
            onClick={toggle}
            className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors duration-150 outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50 ${enabled ? "bg-primary" : "bg-white/10"}`}
          >
            <span className={`pointer-events-none inline-block size-4 transform rounded-full bg-white shadow-sm transition-transform duration-150 ${enabled ? "translate-x-4" : "translate-x-0.5"}`} />
          </button>
        </div>
        {!native && <p className="text-xs text-subtle-foreground">Startup controls require the desktop app.</p>}
        {error && <p role="alert" className="text-xs text-destructive">Could not update startup: {error}</p>}
      </div>
    </section>
  )
}
