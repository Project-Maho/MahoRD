import type * as React from "react"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

export interface Props {
  ip: string;
  pin: string;
  relayUrl?: string;
  onIpChange: (v: string) => void;
  onPinChange: (v: string) => void;
  onRelayUrlChange?: (v: string) => void;
  onConnect: () => void;
  busy?: boolean;
  error?: string | null;
}

export type DirectConnectProps = Props;

export function DirectConnect({
  ip,
  pin,
  relayUrl = "",
  onIpChange,
  onPinChange,
  onRelayUrlChange,
  onConnect,
  busy = false,
  error = null,
}: Props) {
  const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault()
    onConnect()
  }

  return (
    <Card
      aria-labelledby="direct-heading"
      className="p-4 sm:p-5 rounded-panel border-border bg-card gap-4"
    >
      <CardHeader className="p-0 gap-1.5">
        <CardTitle
          id="direct-heading"
          className="text-base font-semibold text-foreground leading-tight"
        >
          Direct connection
        </CardTitle>
        <CardDescription
          id="direct-help"
          className="text-xs text-muted-foreground leading-relaxed"
        >
          Enter a host address. For a new pairing, enter the host's eight-digit PIN.
        </CardDescription>
      </CardHeader>

      <CardContent className="p-0">
        <form
          id="direct-form"
          onSubmit={handleSubmit}
          noValidate
          className="grid grid-cols-1 min-[400px]:grid-cols-[minmax(0,1fr)_minmax(7rem,auto)] min-[1040px]:grid-cols-[minmax(0,1fr)_minmax(10rem,14rem)_minmax(7rem,auto)] gap-3 items-end"
        >
          <div className="col-span-1 min-[400px]:col-span-2 min-[1040px]:col-span-1 space-y-1.5 min-w-0">
            <Label htmlFor="direct-ip">Host address</Label>
            <Input
              id="direct-ip"
              type="text"
              value={ip}
              onChange={(e) => onIpChange(e.target.value)}
              disabled={busy}
              autoComplete="off"
              spellCheck={false}
              aria-describedby="direct-help"
              className="font-mono text-foreground"
            />
          </div>

          <div className="col-span-full space-y-1.5 min-w-0">
            <Label htmlFor="direct-relay">Relay URL</Label>
            <Input
              id="direct-relay"
              type="text"
              value={relayUrl}
              onChange={(e) => onRelayUrlChange?.(e.target.value)}
              disabled={busy}
              autoComplete="off"
              spellCheck={false}
              placeholder="wss://relay.example"
              className="font-mono text-foreground"
            />
          </div>

          <div className="col-span-1 space-y-1.5 min-w-0">
            <Label htmlFor="direct-pin">PIN (optional)</Label>
            <Input
              id="direct-pin"
              type="password"
              inputMode="numeric"
              value={pin}
              onChange={(e) => onPinChange(e.target.value)}
              disabled={busy}
              autoComplete="off"
              aria-describedby="direct-help"
              className="font-mono text-foreground"
            />
          </div>

          <Button
            id="btn-direct-connect"
            type="submit"
            disabled={busy}
            aria-busy={busy}
            className="col-span-1 w-full shrink-0"
          >
            Connect
          </Button>

          {error ? (
            <p
              id="direct-error"
              role="alert"
              className="col-span-full text-sm text-destructive font-medium"
            >
              {error}
            </p>
          ) : null}
        </form>
      </CardContent>
    </Card>
  )
}
