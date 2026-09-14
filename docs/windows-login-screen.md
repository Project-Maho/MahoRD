# Windows login-screen streaming (MahoRDHost service)

Streaming the Windows logon screen, lock screen and UAC consent prompt requires a
host that can follow the console session onto the **secure desktop**
(`WinSta0\Winlogon`). A process running inside a logged-on user session cannot:
DXGI loses access the moment Windows switches away from `WinSta0\Default`.

`maho-host` solves this the way Parsec does — a LocalSystem service supervises the
active console session and keeps one SYSTEM worker alive on whichever desktop
currently owns input.

## Install

```powershell
maho-host.exe --install-service      # registers MahoRDHost (LocalSystem, auto-start) and starts it
maho-host.exe --uninstall-service    # stops, removes, and releases 19730/19731
sc query MahoRDHost                  # expect: STATE : 4  RUNNING
```

The SCM runs `maho-host.exe --service-run`. The supervisor spawns
`maho-host.exe --session-worker` itself — **do not launch the worker by hand while
the service is running**, or it will hold TCP 19730 and the service's own worker
will fail to bind and respawn in a loop.

## How it works

| Component | Session | Role |
|---|---|---|
| `--service-run` | 0 | SCM dispatcher; supervises the console session |
| `--session-worker` | 1 (console) | Captures, encodes, streams, injects input |

Each second the supervisor reads the console session id
(`WTSGetActiveConsoleSessionId`) plus the current input desktop, and reconciles
them against the running worker: spawn when none exists, respawn when the session
or desktop changed, stop when no console session is attached. Workers are created
with `OpenProcessToken` → `DuplicateTokenEx(TokenPrimary)` →
`SetTokenInformation(TokenSessionId)` → `CreateProcessAsUserW`, with `lpDesktop`
set to `WinSta0\Winlogon` or `WinSta0\Default`.

### Why the worker reports the desktop

`OpenInputDesktop` is **session-local**: called from the service in session 0 it
always fails, so it cannot distinguish a locked console from an active one. WTS
lock flags (`WTSQuerySessionInformationW`) were measured and never change on a
console session under autologon, so they are useless here too.

The worker runs *inside* the console session and can see the input desktop, so it
publishes the name to `%ProgramData%\MahoRD\input-desktop.txt` every 500 ms. When
the console switches to the secure desktop the worker's own `OpenInputDesktop`
starts failing — that failure **is** the signal, and it reports `Winlogon` so the
supervisor respawns a worker that has access.

## Pairing store

Service mode uses `%ProgramData%\MahoRD\host-authorizations.json`, not the per-user
store. A LocalSystem process resolves the per-user data directory to
`C:\Windows\System32\config\systemprofile`, which would force already-paired
clients back through PIN bootstrap. Migrate existing records once:

```powershell
Copy-Item "$env:APPDATA\MahoRD\host-authorizations.json" "$env:ProgramData\MahoRD\" -Force
Copy-Item "$env:APPDATA\MahoRD\pairing-keys.json"        "$env:ProgramData\MahoRD\" -Force
```

## Troubleshooting

An SCM-hosted process has no console, so diagnostics go to
`%ProgramData%\MahoRD\service.log`:

```
tick console=ConsoleState { session_id: Some(1), desktop: Default } worker=Some(...) action=Idle
spawned worker pid=9996
```

| Symptom | Cause |
|---|---|
| Worker respawns every second | Worker is dying at startup. Check whether something else already holds 19730, or whether it was spawned onto a desktop DXGI cannot capture. |
| `action=Spawn` repeating with a changing pid | Same as above; the supervisor is healthy, the worker is not. |
| Client falls back to PIN | Service is reading the per-user store; migrate the pairing files above. |
| No listener on 19730 | No worker is running; read `service.log` for the last `action=`. |

## Verified

On `DESKTOP-1LAPJMP` (Windows 11, rustc 1.97):

- `cargo build --release -p maho-host` exit 0; `sc query MahoRDHost` → `STATE : 4 RUNNING`.
- Service in session 0, exactly one worker in session 1, stable across 10+ supervision ticks.
- Uninstall removes the service, terminates the worker and releases both ports with no orphans; reinstall restores the full pipeline.
- `maho-client` decodes 5/5 frames at 3840×1600 H.264, `packet_loss_ratio 0.0`, reconnecting PIN-lessly through the machine-wide store.
- Desktop transition: writing `Winlogon` into the hint file produces
  `action=Respawn { session_id: 1, desktop: Winlogon }`, the worker pid changes
  (confirming `CreateProcessAsUserW` onto the secure desktop ran), and the loop
  self-heals back to `Default` when the worker republishes.

**Not yet verified on this host:** decoded frames captured while a secure desktop is
actually on screen. That console runs under autologon with no physically attached
display, and it refuses every remote route to a secure desktop — `LockWorkStation`
from a user task and from a SYSTEM task, a secure screensaver, `tscon`, and
restoring default UAC prompting all failed to produce one. Verify by pressing
Win+L at the machine, then streaming.
