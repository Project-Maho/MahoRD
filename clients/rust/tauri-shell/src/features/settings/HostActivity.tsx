import { useEffect, useState } from "react";
import { hostGuests, hostApprove, hostDeny, hostKick, hostKickAll, hostGuestPermissions, isNativeAvailable, type HostGuests, type GuestPermissions } from "@/lib/ipc";
import { Button } from "@/components/ui/button";

const empty: HostGuests = { sessions: [], pending: [], events: [], error: null };
const permissionKeys = ["keyboard", "mouse", "gamepad", "clipboard", "audio"] as const;

export function HostActivity({ panel = false }: { panel?: boolean }) {
  const [data, setData] = useState(empty);
  const [error, setError] = useState<string | null>(null);
  const [notices, setNotices] = useState<string[]>([]);
  const [permissions, setPermissions] = useState<Record<string, GuestPermissions>>({});
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!isNativeAvailable()) return;
    let active = true;
    const refresh = async () => {
      try {
        const next = await hostGuests(!panel);
        if (!active || !next) return;
        setData(next);
        setPermissions(Object.fromEntries(next.sessions.filter(guest => guest.permissions).map(guest => [guest.pairing_id, guest.permissions!])));
        if (!panel && next.events.length) setNotices(old => [...old, ...next.events].slice(-4));
        setError(next.error);
      } catch (e) { if (active) setError(e instanceof Error ? e.message : String(e)); }
    };
    void refresh();
    const timer = setInterval(refresh, 1000);
    return () => { active = false; clearInterval(timer); };
  }, [panel]);
  const action = async (run: () => Promise<unknown>) => {
    setBusy(true); setError(null);
    try { const result = await run(); if (result === false) throw new Error("Request expired or guest disconnected"); setData(await hostGuests(!panel)); }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  if (panel) return <section className="space-y-3" aria-label="Connected guests">
    <div className="flex items-center justify-between gap-2"><h2 className="text-sm font-semibold">Connected guests</h2><Button size="sm" variant="secondary" disabled={busy || !data.sessions.length} onClick={() => void action(hostKickAll)}>Kick all</Button></div>
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    {!data.sessions.length && <p className="text-sm text-muted-foreground">No connected guests</p>}
    {data.sessions.map(guest => <div key={guest.id} className="space-y-3 rounded-lg border border-border p-3">
      <div className="flex items-start justify-between gap-2"><div><p className="font-medium break-all">{guest.name}</p><p className="text-xs text-muted-foreground">{guest.trust} · connected {guest.connected_seconds}s ago</p></div><Button variant="secondary" size="sm" disabled={busy} onClick={() => void action(() => hostKick(guest.id))}>Kick</Button></div>
      <p className="text-xs text-muted-foreground">Permission changes apply to this guest's pairing. Host policy remains the ceiling.</p>
      <div className="flex flex-wrap gap-3">{permissionKeys.map(key => <label className="flex gap-1 text-xs capitalize" key={key}><input type="checkbox" disabled={busy || !permissions[guest.pairing_id]} checked={permissions[guest.pairing_id]?.[key] ?? false} onChange={event => {
        const next = { ...(permissions[guest.pairing_id] ?? { keyboard: false, mouse: false, gamepad: false, clipboard: false, audio: false }), [key]: event.target.checked };
        void action(async () => { await hostGuestPermissions(guest.pairing_id, next); setPermissions(old => ({ ...old, [guest.pairing_id]: next })); });
      }} />{key}</label>)}</div>
    </div>)}
  </section>;
  return <>
    {!!notices.length && <div role="status" className="fixed bottom-4 right-4 z-[1500] max-w-[calc(100%-32px)] rounded-lg border border-border bg-background p-3 text-sm">{notices.map((notice, i) => <p key={i}>{notice}</p>)}<Button variant="ghost" size="sm" onClick={() => setNotices([])}>Dismiss</Button></div>}
    {!!data.pending.length && <div className="fixed inset-0 z-[1600] grid place-items-center bg-black/60 p-4"><div role="dialog" aria-modal="true" aria-labelledby="approval-heading" className="w-full max-w-md space-y-4 rounded-xl border border-border bg-background p-5 text-foreground"><h2 id="approval-heading" className="font-semibold">Connection requests</h2><p className="text-sm text-muted-foreground">Approve once for an unknown guest. Owner access always reconnects without a PIN.</p>{data.pending.map(request => <div key={request.id} className="space-y-2"><p className="break-all font-medium">{request.name}</p><div className="flex flex-wrap gap-2"><Button autoFocus disabled={busy} onClick={() => void action(() => hostApprove(request.id, false))}>Approve once</Button><Button variant="secondary" disabled={busy} onClick={() => void action(() => hostApprove(request.id, true))}>Approve as owner</Button><Button variant="ghost" disabled={busy} onClick={() => void action(() => hostDeny(request.id))}>Deny</Button></div></div>)}{error && <p role="alert" className="text-sm text-destructive">{error}</p>}</div></div>}
  </>;
}
