import { ArrowRight, Hash, KeyRound, Monitor, Trash2 } from "lucide-react"
import { Card } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

export interface Host {
  id: string;
  name: string;
  ip: string;
  os?: string | null;
  online?: boolean | null;
  paired?: boolean | null;
}

export interface Props {
  hosts: Array<{
    id: string;
    name: string;
    ip: string;
    os?: string | null;
    online?: boolean | null;
    paired?: boolean | null;
  }>;
  busy?: boolean;
  onConnect: (host: { id: string; ip: string; name: string }) => void;
  /** Present only where the page owns the pairing-removal flow. */
  onRemove?: (host: { id: string; ip: string; name: string }) => void;
  removeBusy?: boolean;
  statusMessage?: string | null;
}

export type HostGridProps = Props;

const CHIP = "inline-flex h-6 items-center gap-1.5 rounded-full px-2 text-xs font-medium ring-1 ring-inset"
const CHIP_NEUTRAL = "bg-white/[0.05] text-muted-foreground ring-white/[0.06]"

export function HostGrid({
  hosts,
  busy = false,
  onConnect,
  onRemove,
  removeBusy = false,
  statusMessage,
}: Props) {
  return (
    <section className="space-y-3" aria-labelledby="library-heading">
      <h2
        id="library-heading"
        className="flex items-center gap-2 text-[13px] font-medium text-muted-foreground"
      >
        Listed computers{" "}
        <span
          id="library-count"
          className="inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-white/[0.07] px-1.5 text-xs font-medium tabular-nums text-muted-foreground"
        >
          {hosts.length}
        </span>
      </h2>

      {statusMessage ? (
        <div
          id="library-status"
          role="status"
          className="flex flex-col items-center justify-center gap-3 rounded-panel border border-dashed border-white/10 bg-white/[0.015] px-6 py-12 text-center"
        >
          <div className="grid size-10 place-items-center rounded-[10px] surface-raised">
            <Monitor className="size-5 text-subtle-foreground" aria-hidden="true" />
          </div>
          <p className="max-w-sm text-sm text-muted-foreground">{statusMessage}</p>
        </div>
      ) : null}
      {!statusMessage && hosts.length > 0 && [true, false].map(online => {
        const sectionHosts = hosts.filter(host => (host.online === true) === online)
        if (!online && !sectionHosts.length) return null
        return <section key={String(online)} aria-labelledby={online ? "available-heading" : "offline-heading"} className="space-y-3">
          <h3 id={online ? "available-heading" : "offline-heading"} className="text-sm font-medium text-muted-foreground">
            {online ? "Available" : "Offline"} <span id={online ? "available-count" : "offline-count"}>{sectionHosts.length}</span>
          </h3>
          {online && !sectionHosts.length && hosts.length > 0 && <p className="text-sm text-muted-foreground">No computers are available right now.</p>}
        <div
          id={online ? "host-grid" : "host-grid-offline"}
          role="list"
          aria-busy={busy}
          className="grid grid-cols-[repeat(auto-fill,minmax(min(17rem,100%),1fr))] gap-3"
        >
          {sectionHosts.map((host) => {
            const available = host.online === true
            const canConnect = available && !busy
            const showRemove = Boolean(onRemove)

            return (
              <Card
                key={host.id}
                role="listitem"
                data-host-id={host.id}
                data-availability={available ? "available" : "offline"}
                className="group relative gap-4 p-4 transition-[border-color,background-color] duration-150 hover:border-border-strong hover:bg-surface-2 focus-within:border-border-strong"
              >
                <div className="flex min-w-0 items-start gap-3">
                  <div className="grid size-10 shrink-0 place-items-center rounded-[10px] surface-raised">
                    <Monitor
                      className={cn("size-5", available ? "text-foreground" : "text-subtle-foreground")}
                      aria-hidden="true"
                    />
                  </div>
                  <div className="min-w-0 flex-1 pt-0.5">
                    <h3
                      className="line-clamp-2 break-all text-[15px] font-semibold leading-5 text-foreground"
                      title={host.name}
                    >
                      {host.name}
                    </h3>
                    <p className="mt-0.5 truncate select-text font-mono text-[12.5px] text-muted-foreground">
                      {host.ip}
                    </p>
                  </div>
                </div>

                <div className="flex flex-wrap items-center gap-1.5">
                  <span
                    className={cn(
                      CHIP,
                      available ? "bg-success/10 text-success ring-success/20" : CHIP_NEUTRAL
                    )}
                  >
                    <span
                      aria-hidden="true"
                      className={cn(
                        "size-1.5 rounded-full",
                        available
                          ? "bg-success shadow-[0_0_0_3px_rgb(90_209_154_/_0.16)]"
                          : "bg-subtle-foreground"
                      )}
                    />
                    {available ? "Available" : "Offline"}
                  </span>
                  <span className={cn(CHIP, CHIP_NEUTRAL)}>
                    {host.paired ? (
                      <KeyRound className="size-3" aria-hidden="true" />
                    ) : (
                      <Hash className="size-3" aria-hidden="true" />
                    )}
                    {host.paired ? "Saved pairing" : "PIN may be needed"}
                  </span>
                </div>

                <div className="mt-auto flex items-center justify-between gap-3 border-t border-white/[0.06] pt-3">
                  <span className="truncate text-xs text-subtle-foreground">
                    {host.os || "Unknown OS"}
                  </span>
                  <div className="flex shrink-0 items-center gap-1.5">
                    {showRemove && (
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        data-action="remove-host"
                        disabled={removeBusy}
                        onClick={() => onRemove?.({ id: host.id, ip: host.ip, name: host.name })}
                        title="Remove this computer from the list"
                        className="gap-1.5 hover:bg-destructive/10 hover:text-destructive"
                      >
                        <Trash2 aria-hidden="true" />
                        Remove
                      </Button>
                    )}
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      data-action="connect"
                      disabled={!canConnect}
                      onClick={() => onConnect({ id: host.id, ip: host.ip, name: host.name })}
                      title={
                        !available
                          ? "Offline discovery record. Use Direct connection to try an address explicitly."
                          : undefined
                      }
                      className={cn(
                        "gap-1.5",
                        canConnect &&
                          "group-hover:border-transparent group-hover:bg-primary group-hover:text-primary-foreground group-hover:shadow-cta group-focus-within:border-transparent group-focus-within:bg-primary group-focus-within:text-primary-foreground group-focus-within:shadow-cta hover:border-transparent hover:bg-primary-hover"
                      )}
                    >
                      Connect
                      <ArrowRight className="size-3.5" aria-hidden="true" />
                    </Button>
                  </div>
                </div>
              </Card>
            );
          })}
        </div>
        </section>
      })}
    </section>
  );
}

export default HostGrid;
