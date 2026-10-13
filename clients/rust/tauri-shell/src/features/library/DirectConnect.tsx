import type * as React from "react"
import { ArrowRight, CircleAlert, LoaderCircle } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

export interface Props {
  ip: string;
  pin: string;
  onIpChange: (v: string) => void;
  onPinChange: (v: string) => void;
  onConnect: () => void;
  busy?: boolean;
  error?: string | null;
}

export type DirectConnectProps = Props;

export function DirectConnect({
  ip,
  pin,
  onIpChange,
  onPinChange,
  onConnect,
  busy = false,
  error = null,
}: Props) {
  const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault()
    onConnect()
  }

  return (
    <section aria-labelledby="direct-heading" className="space-y-3">
      <h2 id="direct-heading" className="text-[13px] font-medium text-muted-foreground">
        Direct connection
      </h2>
      <p id="direct-help" className="text-xs text-subtle-foreground">
        Leave PIN blank to use the saved pairing for this computer. New computers need the host's eight-digit PIN.
      </p>

      <form
        id="direct-form"
        onSubmit={handleSubmit}
        noValidate
        className="grid grid-cols-1 gap-2 rounded-panel surface-card p-2 min-[560px]:grid-cols-[minmax(0,1fr)_10rem_auto]"
      >
        <Label htmlFor="direct-ip" className="sr-only">
          Host address
        </Label>
        <Input
          id="direct-ip"
          type="text"
          value={ip}
          onChange={(e) => onIpChange(e.target.value)}
          disabled={busy}
          autoComplete="off"
          spellCheck={false}
          placeholder="Host address"
          aria-describedby="direct-help"
          className="font-mono"
        />
        <Label htmlFor="direct-pin" className="sr-only">
          PIN (optional)
        </Label>
        <Input
          id="direct-pin"
          type="password"
          inputMode="numeric"
          value={pin}
          onChange={(e) => onPinChange(e.target.value)}
          disabled={busy}
          autoComplete="off"
          placeholder="PIN (optional)"
          aria-describedby="direct-help"
          className="font-mono"
        />
        <Button
          id="btn-direct-connect"
          type="submit"
          size="lg"
          disabled={busy}
          aria-busy={busy}
        >
          {busy ? (
            <LoaderCircle className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
          ) : null}
          {busy ? "Connecting" : "Connect"}
          {busy ? null : <ArrowRight aria-hidden="true" />}
        </Button>

        {error ? (
          <p
            id="direct-error"
            role="alert"
            className="col-span-full flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/10 px-3 py-2 text-[13px] text-destructive"
          >
            <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <span>{error}</span>
          </p>
        ) : null}
      </form>
    </section>
  )
}
