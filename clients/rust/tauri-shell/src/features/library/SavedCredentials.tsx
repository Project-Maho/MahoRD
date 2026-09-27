import { KeyRound, RefreshCw, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"

export interface SavedCredential {
  id: string;
  name: string;
  endpoint?: string | null;
}

export interface Props {
  pairings: Array<{ id: string; name: string; endpoint?: string | null }>;
  busy?: boolean;
  onConnect: (id: string) => void;
  onForget: (id: string) => void;
  onRefresh: () => void;
}

export type SavedCredentialsProps = Props;

export function SavedCredentials({
  pairings,
  busy = false,
  onConnect,
  onForget,
  onRefresh,
}: Props) {
  return (
    <section
      id="saved-pairings-section"
      data-testid="saved-credentials"
      aria-labelledby="saved-pairings-heading"
      className="space-y-3"
    >
      <div className="flex items-center justify-between gap-4">
        <h2
          id="saved-pairings-heading"
          className="flex items-center gap-2 text-[15px] font-semibold leading-5 tracking-[-0.01em] text-foreground"
        >
          Saved credentials{" "}
          <span
            id="saved-pairings-count"
            className="inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-white/[0.07] px-1.5 text-xs font-medium tabular-nums text-muted-foreground"
          >
            {pairings.length}
          </span>
        </h2>
        <Button
          id="btn-refresh-pairings"
          variant="ghost"
          size="icon-sm"
          onClick={onRefresh}
          disabled={busy}
          aria-label="Refresh saved credentials"
          title="Refresh saved credentials"
        >
          <RefreshCw aria-hidden="true" />
        </Button>
      </div>

      <p className="text-xs text-subtle-foreground">
        Saved credentials reconnect without a PIN. Same-name entries are told apart by ID and endpoint.
      </p>

      {pairings.length === 0 ? (
        <div
          id="saved-pairings-empty"
          className="flex items-center gap-3 rounded-panel border border-dashed border-white/10 bg-white/[0.015] px-4 py-4"
        >
          <KeyRound className="size-4 shrink-0 text-subtle-foreground" aria-hidden="true" />
          <div className="text-sm">
            <p className="text-muted-foreground">No saved credentials yet.</p>
            <p className="text-xs text-subtle-foreground">Connect once with a host PIN to save one.</p>
          </div>
        </div>
      ) : (
        <div
          id="saved-pairings-list"
          role="list"
          className="divide-y divide-white/[0.06] overflow-hidden rounded-panel surface-card"
        >
          {pairings.map((pairing) => (
            <div
              key={pairing.id}
              data-pairing-id={pairing.id}
              role="listitem"
              className="flex items-center gap-3 px-4 py-3 transition-colors duration-150 hover:bg-white/[0.02] max-[520px]:flex-wrap"
            >
              <div className="grid size-8 shrink-0 place-items-center rounded-[9px] surface-raised">
                <KeyRound className="size-4 text-brand-soft" aria-hidden="true" />
              </div>
              <div className="min-w-0 flex-1">
                <h3
                  className="truncate text-sm font-semibold leading-5 text-foreground"
                  title={pairing.name}
                >
                  {pairing.name}
                </h3>
                <p className="break-all select-text font-mono text-xs text-muted-foreground">
                  {pairing.endpoint || "Endpoint not yet recorded"}
                </p>
                <p className="truncate select-text font-mono text-[11px] text-subtle-foreground">
                  ID {pairing.id}
                </p>
              </div>
              <div className="flex shrink-0 items-center gap-1.5 max-[520px]:w-full max-[520px]:justify-end">
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={busy}
                  onClick={() => onConnect(pairing.id)}
                  data-action="connect-saved"
                >
                  Connect
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  onClick={() => onForget(pairing.id)}
                  data-action="forget-saved"
                  className="hover:bg-destructive/10 hover:text-destructive"
                >
                  <Trash2 aria-hidden="true" />
                  Forget
                </Button>
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  )
}
