import { useEffect, useRef, useState } from "react"
import { Check, Copy, Pause, Play, ScreenShare } from "lucide-react"
import { Card } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

export interface ThisComputerCardProps {
  ip: string | null;
  pin: string | null;
  running: boolean;
  busy?: boolean;
  onToggleSharing: () => void;
  onCopyInfo: () => void | boolean | Promise<void | boolean>;
}

export type Props = ThisComputerCardProps;

export function ThisComputerCard({
  ip,
  pin,
  running,
  busy = false,
  onToggleSharing,
  onCopyInfo,
}: ThisComputerCardProps) {
  const displayIp = ip || "Unavailable"
  const displayPin = pin || "--------"
  const [copied, setCopied] = useState(false)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current)
    }
  }, [])

  const handleCopy = async () => {
    const ok = await onCopyInfo()
    if (ok === true) {
      setCopied(true)
      if (timerRef.current) clearTimeout(timerRef.current)
      timerRef.current = setTimeout(() => setCopied(false), 1600)
    }
  }

  return (
    <Card
      id="this-computer-card"
      className="flex-row items-center justify-between gap-4 p-4 pl-5 max-[720px]:flex-col max-[720px]:items-stretch max-[720px]:gap-4"
    >
      <div className="flex min-w-0 items-center gap-3.5">
        <div className="grid size-10 shrink-0 place-items-center rounded-[10px] surface-raised">
          <ScreenShare
            className={cn("size-5", running ? "text-foreground" : "text-subtle-foreground")}
            aria-hidden="true"
          />
        </div>
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2
              id="this-computer-title"
              className="text-[15px] font-semibold leading-5 tracking-[-0.01em] text-foreground"
            >
              This Computer (Host)
            </h2>
            <span
              id="this-computer-badge"
              className={cn(
                "inline-flex h-6 items-center gap-1.5 rounded-full px-2 text-xs font-medium ring-1 ring-inset",
                running
                  ? "bg-success/10 text-success ring-success/20"
                  : "bg-warning/10 text-warning ring-warning/20"
              )}
            >
              <span
                aria-hidden="true"
                className={cn(
                  "size-1.5 rounded-full",
                  running
                    ? "bg-success shadow-[0_0_0_3px_rgb(90_209_154_/_0.16)]"
                    : "bg-warning"
                )}
              />
              <span className="inline-block lowercase first-letter:uppercase">
                {running ? "READY" : "UNAVAILABLE"}
              </span>
            </span>
          </div>
          <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px] text-muted-foreground">
            <span id="this-computer-ip">
              <span className="text-subtle-foreground">IP </span>
              <span className="select-text font-mono text-foreground">{displayIp}</span>
            </span>
            <span className="text-subtle-foreground max-[720px]:hidden" aria-hidden="true">
              ·
            </span>
            <span className="inline-flex items-center gap-1.5">
              <span className="text-subtle-foreground">PIN</span>
              <strong
                id="this-computer-pin"
                className="select-text rounded-md bg-well px-1.5 py-0.5 font-mono text-[13px] font-medium tracking-[0.08em] text-foreground ring-1 ring-white/[0.08]"
              >
                {displayPin.length === 8 ? (
                  <>
                    <span>{displayPin.slice(0, 4)}</span>
                    <span className="ml-1">{displayPin.slice(4)}</span>
                  </>
                ) : (
                  displayPin
                )}
              </strong>
            </span>
          </p>
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-2 max-[720px]:w-full">
        <Button
          id="btn-host-toggle"
          variant="secondary"
          size="sm"
          className="max-[720px]:flex-1"
          onClick={onToggleSharing}
          disabled={busy}
        >
          {running ? <Pause aria-hidden="true" /> : <Play aria-hidden="true" />}
          {running ? "Pause Sharing" : "Resume Sharing"}
        </Button>
        <Button
          id="btn-host-copy"
          variant="secondary"
          size="sm"
          className="max-[720px]:flex-1"
          onClick={handleCopy}
          disabled={busy}
        >
          {copied ? (
            <Check className="text-success" aria-hidden="true" />
          ) : (
            <Copy aria-hidden="true" />
          )}
          {copied ? "Copied" : "Copy Info"}
        </Button>
      </div>
    </Card>
  )
}
