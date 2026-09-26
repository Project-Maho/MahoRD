# BUG: Linux host never tears down the capture/encode pipeline when a client→host clipboard sync blocks the session thread

**Severity:** P1 — permanent resource leak (GPU video encoder + 3 threads + zombie child + `wl-copy` daemon), silent (no log), survives session death, and survives every subsequent reconnect attempt.
**Component:** `clients/rust/maho-host` — `clipboard_linux.rs` (primary), `session.rs` (secondary, teardown design)
**Platform:** Linux + Wayland (Hyprland), `wl-clipboard` installed
**Host build:** `/home/indo/.local/share/MahoRD/releases/20260924-teardownfix/maho-host` (HEAD `c086a5a`, `maho-host` crate `0.1.0`)
**Reported:** 2026-09-25 · observed on a live daemon that had been leaking for 13h

---

## 1. Summary

With **no client connected**, the host daemon kept a full capture + encode pipeline running for
13 hours: `maho-linux-capture`, `maho-linux-encode` and `maho-linux-audio` threads plus an
anonymous UDP sender thread stayed alive, the AMD VAAPI encoder ran at ~50 % duty, the compositor
was forced to render at 60 fps (~42 % GPU), and the sender kept blasting ~1350 UDP datagrams/s
(5.4 Mbit/s) at a peer that had been gone for 13 hours.

Root cause is a **blocking read on a pipe whose write end is inherited by a grandchild process**:
`clipboard_linux.rs::apply_remote_text()` spawns `wl-copy` and then calls
`child.wait_with_output()`. On Wayland, `wl-copy` forks a daemon to serve the selection; that
daemon inherits the stderr pipe, so the pipe never reaches EOF, and `wait_with_output()` never
returns. The session thread blocks inside the packet loop, so:

1. it stops servicing the TCP stream (pings included) — the client looks stalled to the watchdog;
2. the watchdog's only remedy is `cloned_tcp.shutdown(Both)`, which **cannot** unblock a thread
   parked in `read(2)`;
3. `handle_connection` therefore never returns, `media_handle` (a local `Option<Box<dyn MediaHandle>>`)
   is never dropped, `Workers::stop()` never runs, and the whole media pipeline leaks;
4. the leaked sender thread keeps draining the media channel, so the encoder never blocks and keeps
   encoding into a void.

There is no log line: the caller only warns on `Err`, and the call never returns at all.

## 2. Impact

- **GPU burn while idle:** video encoder ~49 % duty and compositor ~42 % duty with zero clients.
  Measured on AMD RX 580: `drm-engine-enc_1` ≈ 494 ms per second of wall time, `drm-engine-gfx` ≈ 422 ms/s.
- **Bandwidth burn:** ~0.68 MB/s (5.4 Mbit/s, ~1349 datagrams/s) continuously sent to a dead peer.
- **Process litter:** one zombie child (`wl-copy <defunct>`, unreaped because `wait()` is never
  reached) and one `wl-copy` daemon that keeps ownership of the Wayland clipboard selection —
  so the host's clipboard stays pinned to a 13-hour-old value.
- **Not self-healing:** the wedge is permanent until the daemon is restarted; reconnects create new
  session threads while the old pipeline keeps running (each wedged session leaks another pipeline).
- **Silent:** no error, no warning, no metric. The only externally visible symptom is GPU/CPU load
  with no client connected.

## 3. Environment

| | |
|---|---|
| OS | Arch Linux, kernel 7.1.9-arch1-2 |
| Desktop | Hyprland (Wayland), `WAYLAND_DISPLAY=wayland-1`, output `HDMI-A-2` 3840x1600@60 |
| GPU | AMD Radeon RX 580 (Ellesmere, amdgpu), 8 GiB — VAAPI encode (`drm-engine-enc_1`) |
| Clipboard tools | `wl-clipboard` (`wl-copy`, `wl-paste`) present |
| Service | `maho-host.service` (systemd --user), `--bootstrap-pin … --auto-approve --output HDMI-A-2` |
| Client | `MahoRD-Tauri` on macOS (`indo-macbookpro`, 100.78.73.127), capability `TEXT_CLIPBOARD_SYNC` |

## 4. Observed timeline (live daemon, PID 3459757, started 2026-09-24 23:27:46)

| Time (KST) | Event |
|---|---|
| 09-25 09:55:40 | Session threads spawn for the macOS client; handshake completes; log: `v3 handshake authenticated; UDP ciphers armed client="MahoRD-Tauri"`, `Starting UDP sender thread peer=100.78.73.127:52829` |
| 09-25 10:13:10 | `wl-copy` (pid 4109707) and its forked daemon (pid 4109709) are created — the client sent clipboard text (ClientToHost sync) |
| 09-25 10:13:11 | `WARN session watchdog aborted a stalled session peer=100.78.73.127:56161` — the watchdog's socket shutdown cannot unblock the pipe read |
| 09-25 10:13:41 | `WARN session watchdog aborted a stalled session peer=100.78.73.127:53093 idle=30.274s` |
| 09-25 23:20 | Still leaking: capture thread 20,559 s CPU, encode thread 22,450 s CPU, `wl-copy` daemon alive, zombie unreaped, 1349 UDP datagrams/s to the dead peer |

The two events at 10:13:10/10:13:11 are one second apart — the clipboard write is what stops the
session, and the watchdog fires immediately after because session progress stops with it.

## 5. Reproduction

1. Start `maho-host` on Linux/Wayland with `wl-clipboard` installed.
2. Pair a client that advertises `Capabilities::TEXT_CLIPBOARD_SYNC` and complete the v3 handshake.
3. From the client, copy a short text to the clipboard (client→host sync).
4. Kill the client (or let it disconnect).
5. Observe on the host:
   - `ps -L -o tid,comm,etime,time -p <host-pid>` → `maho-linux-capture`, `maho-linux-encode`,
     `maho-linux-audio` threads still alive after the session is gone;
   - `/proc/<host-pid>/task/<tid>/wchan` for the `maho-host-session` thread → `anon_pipe_read`;
   - `ps --ppid <host-pid>` → a `wl-copy <defunct>` zombie plus a reparented live `wl-copy`;
   - the GPU encoder keeps working and UDP keeps flowing (commands in §8).

Deterministic on this machine: the wedge reproduced on the first client→host clipboard sync, and the
leak persisted for 13 h across the client's disappearance.

## 6. Root cause

### 6.1 Primary: `wait_with_output()` on a pipe a grandchild still holds

`clients/rust/maho-host/src/clipboard_linux.rs:192`

```rust
pub fn apply_remote_text(&mut self, text: &str, now: Instant) -> Result<(), ClipboardError> {
    ...
    let mut command = match self.backend {
        ClipboardBackend::Wayland => {
            let mut command = Command::new("wl-copy");
            command.args(["--type", "text/plain;charset=utf-8"]);
            command
        }
        ...
    };
    command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    child.stdin.take()... .write_all(text.as_bytes())?;
    let output = child.wait_with_output()?;      // clipboard_linux.rs:218  <-- never returns
    ...
}
```

`wait_with_output()` drains stdout and stderr **to EOF** before reaping. `wl-copy` on Wayland forks a
daemon that keeps serving the selection and that daemon inherits the stderr pipe's write end, so EOF
never arrives. Verified on the live daemon:

```
/proc/3459757/fd/51           -> pipe:[125114368]      (host: read end, held by the session thread)
/proc/4109709/fd/2  (wl-copy) -> pipe:[125114368]      (forked daemon: write end, still open)
/proc/3459757/task/4087004/wchan -> anon_pipe_read     (maho-host-session, blocked)
/proc/3459757/task/4087004/syscall -> 0 (read)         fd 0x33 = 51
```

Note the asymmetry inside the same file: the **read** path is bounded —
`read_command_bounded_within()` (`clipboard_linux.rs:244`, 500 ms deadline via `READ_COMMAND_TIMEOUT`
at `clipboard_linux.rs:31`, poll + kill + reap). The **write** path has no deadline at all.

### 6.2 Why the session thread's death is the only teardown

`clients/rust/maho-host/src/session.rs`

- `2827`: `let mut media_handle: Option<Box<dyn MediaHandle>> = None;` — a local of `handle_connection` (`2804`).
- `3234`: `media_handle = Some(self.media_source.start(media_tx)?);` — pipeline starts on handshake ack.
- `3395`: `clipboard.apply_remote_text(&update.text, Instant::now())` — called **inside the packet loop**
  (client→host `ClipboardSyncUpdate`), with only a `warn!` on `Err`; a hang is invisible.

So the pipeline's lifetime is tied to one stack frame. Cleanup depends on
`LinuxMediaHandle`/`Workers` `Drop` (`session.rs:2318`, `native_pipeline.rs:209` `stop()` → `handoff.stop()`
+ join, `native_pipeline.rs:221` `Drop`), which never runs if the frame never unwinds.

### 6.3 Why the watchdog cannot recover it

`session.rs:2695-2696`

```rust
let _ = cloned_tcp.shutdown(std::net::Shutdown::Both);
watchdog_watch.abort();
break;
```

The watchdog (`SESSION_STALL_TIMEOUT` = 30 s, `session.rs:60`) only shuts down the socket. A thread
blocked in `read(2)` on an unrelated pipe never observes that, so `serve_connection`/`handle_connection`
never return and nothing else is torn down. There is no supervisor-side handle to stop the pipeline.

### 6.4 Why the leak keeps costing GPU

The sender thread (`session.rs:2925`) loops on `receiver.recv_timeout(5ms)`, so the media channel is
always drained. The encoder therefore never blocks on a full channel (`sync_channel(16)`,
`session.rs:3233`) and keeps
encoding every captured frame at full rate, and the capture thread keeps forcing the compositor to
produce frames (`zwlr_screencopy_manager_v1`). UDP is fire-and-forget, so sending to a dead peer never
fails either.

### 6.5 Causal proof (reversible experiment)

Sending `SIGSTOP` to only the `maho-linux-capture` thread and sampling per-fd `fdinfo` counters:

```
capture running     maho enc 494 ms/s (49%)   Hyprland gfx 422 ms/s (42%)   UDP out 1349/s
capture SIGSTOPped  maho enc   0.7 ms/s (0.1%) Hyprland gfx   2.8 ms/s (0.3%) UDP out  244/s
after SIGCONT       maho enc 596 ms/s (60%)   Hyprland gfx 507 ms/s (51%)   UDP out 1517/s
```

The idle GPU load is entirely the leaked pipeline; with capture stopped the compositor is idle (0.3 %).

## 7. Suggested fix

**Primary (required):** never wait for EOF on a pipe a forked grandchild can inherit.

- Simplest correct version: use `stderr(Stdio::null())` on the write path (the stderr text is only used
  for the error message today), or redirect it to a temp file, and reap the child with a bounded wait.
- Better: mirror the read path — a bounded reader with a deadline plus `kill()` + `wait()` on timeout
  (reuse/extend `read_command_bounded_within`), so a wedged `wl-copy` is killed and reported instead of
  hanging the session.
- Either way the child must be reaped even on the timeout path (today the parent stays a zombie because
  `wait()` is never reached).

**Secondary (defense in depth, recommended):** decouple pipeline lifetime from the session thread's
stack frame.

- Give the supervisor (accept loop and/or watchdog) a shared handle to the media pipeline
  (`Arc<Mutex<Option<Box<dyn MediaHandle>>>>` or an atomic stop flag the supervisor can trip), so a
  session that is declared dead stops its pipeline even if its thread is wedged in a syscall.
- Consider an idle reaper: if a session's `finished` flag is set, stop its pipeline from outside.
- A blocked session thread should also be visible: log/emit when `apply_remote_text` exceeds its deadline
  (today a hang is completely silent — no warning, no metric).

## 8. Detection commands (for future triage)

```bash
# GPU work per process (gpu_busy_percent does NOT reflect encoder load on amdgpu)
for fd in /proc/<host-pid>/fd/*; do cat /proc/<host-pid>/fdinfo/$(basename $fd) 2>/dev/null | grep drm-engine-; done

# encoder duty (%/10) and compositor duty, sampled
#   drm-engine-enc_1: 494 ms/s == ~49 %

# bytes/UDP flowing to a peer that should be gone
awk '/^Udp:/{print $5}' /proc/net/snmp            # OutDatagrams (field 5)
cat /sys/class/net/<iface>/statistics/tx_bytes    # e.g. tailscale0

# leaked threads / blocked session thread
ps -L -o tid,comm,etime,time -p <host-pid>
cat /proc/<host-pid>/task/<tid>/wchan             # anon_pipe_read == clipboard child pipe

# process litter
ps --ppid <host-pid> -o pid,stat,etime,comm       # Z = unreaped wl-copy, S = forked daemon
```

## 9. Workaround

`systemctl --user restart maho-host.service` clears the leak (threads, zombie, `wl-copy` daemon,
GPU load). Side effect: the `wl-copy` daemon owned the Wayland clipboard selection for 13 h, so the
selection owner changes and the stale clipboard value is lost.

---

### Evidence snapshot (live daemon, 2026-09-25 23:20–23:55 KST)

```
MainPID=3459757  ActiveEnterTimestamp=Thu 2026-09-24 23:27:47 KST  NRestarts=0
VmRSS 106 MB   Threads 17

tid=4087004  maho-host-sessi  wchan=anon_pipe_read   syscall=read(51)   <- wedged session
tid=4087009  maho-linux-capt  cpu 20559 s            ~47 % CPU
tid=4087010  maho-linux-enco  cpu 22450 s            ~50 % CPU
tid=4087011  maho-linux-audi  cpu   139 s
(anonymous UDP sender thread)                        udp_out 1349/s

fd 29  drm-engine-enc_1  2000 ms / 4 s   (~50 % duty)
fd 29  drm-engine-compute 102 ms / 4 s

ps --ppid 3459757:
  4109707  Z  wl-copy <defunct>                        (unreaped, wait() never reached)
  4109709  S  wl-copy --type text/plain;charset=utf-8  (reparented to systemd --user, holds fd 2 -> pipe:[125114368])
  4087013  S  pw-record --raw --rate 48000 …           (leaked audio capture child, `audio_linux.rs:75`)
```
