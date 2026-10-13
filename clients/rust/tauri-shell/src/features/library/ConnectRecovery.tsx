import { Button } from "@/components/ui/button"
import type { ConnectionInstance, ValidateConnectionOptions } from "@/lib/connection"

// App unmounts the launcher while connecting; retain the attempt in memory
// with the shared connection, never in storage (it can contain a PIN).
export const connectionAttempts = new WeakMap<ConnectionInstance, ValidateConnectionOptions>()

export const CONNECT_RECOVERY: Record<string, { message: string; action?: "pin" | "forget" }> = {
  "pairing-required": { message: "This computer needs authorization. Enter its PIN for a new connection.", action: "pin" },
  "pairing-denied": { message: "The host declined the connection. Ask its owner to approve access, then retry." },
  "pairing-locked-out": { message: "Pairing is temporarily locked after too many attempts. Wait for the host to unlock it, then retry." },
  "pairing-disabled": { message: "The host is not accepting new pairings. Ask its owner to open pairing, then retry." },
  "credential-rejected": { message: "The host rejected the saved credential. Check the host's authorization before explicitly forgetting this pairing.", action: "forget" },
  "invalid-pin": { message: "The PIN was rejected. Enter the host's current eight-digit PIN.", action: "pin" },
  "consent-timeout": { message: "Host approval timed out. Ask the owner to approve the next request, then retry." },
  "handshake-timeout": { message: "The host did not respond in time. Check that MahoRD is running and the network is reachable, then retry." },
  "remote-closed": { message: "The host closed the connection. Check its hosting status, then retry." },
  "network-unreachable": { message: "Cannot reach this computer. Check its power, network or Tailscale connection and MahoRD host, then retry." },
  "cleanup-failed": { message: "The previous session could not be closed. Retry cleanup before connecting again." },
  "cancelled": { message: "The connection was cancelled. Retry to connect to the same computer." },
  "connection-failed": { message: "Could not establish a connection. Check the host and network, then retry." },
  "incompatible-peer": { message: "The host and client are incompatible. Update MahoRD on both computers, then retry." },
}

export function ConnectRecovery({ error, code, paired, busy, onRetry, onPin, onForget, onCleanup }: {
  error: string | null; code?: string | null; paired?: boolean; busy?: boolean;
  onRetry: () => void; onPin?: () => void; onForget?: () => void; onCleanup?: () => void;
}) {
  if (!error) return null
  const recovery = CONNECT_RECOVERY[code || ""]
  return <section role="alert" className="space-y-3 rounded-panel surface-card p-4">
    <p className="text-sm text-foreground">{paired && code === "pairing-required" ? "The saved pairing could not be found. Retry without erasing your credentials; check authorization on the host." : recovery?.message || "Connection failed. Check the host and retry."}</p>
    <details className="text-xs text-muted-foreground"><summary>Technical details{code ? ` (${code})` : ""}</summary>{error}</details>
    <div className="flex flex-wrap gap-2">
      <Button size="sm" disabled={busy} onClick={onRetry} data-action="retry-connect">Retry</Button>
      {recovery?.action === "pin" && !paired && onPin && <Button size="sm" variant="secondary" onClick={onPin}>Enter PIN</Button>}
      {recovery?.action === "forget" && paired && onForget && <Button size="sm" variant="secondary" disabled={busy} onClick={onForget}>Forget pairing…</Button>}
      {code === "cleanup-failed" && onCleanup && <Button size="sm" variant="secondary" disabled={busy} onClick={onCleanup}>Retry cleanup</Button>}
    </div>
  </section>
}
