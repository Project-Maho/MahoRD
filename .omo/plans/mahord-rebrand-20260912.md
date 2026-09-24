# MahoRD Rebrand — Full Replacement Plan (2026-09-12)

Status: executing. Tier: HEAVY. Lead: coordinator/reviewer/verifier; implementation delegated to `mahoquot/gemini-3.8-flash-high` (category `quick`) via mass-ulw. Review gate: `opencodex/gpt-6-astra` (user-mandated).

## Mapping table (authoritative — paste into every sweep node prompt)

| Old | New |
|---|---|
| `EclipticRD` | `MahoRD` |
| `eclipticrd` | `mahord` |
| `erd-proto` / `erd-net` / `erd-decode` / `erd-render` / `erd-app` / `erd-host` / `erd-mobile` | `maho-proto` / `maho-net` / `maho-decode` / `maho-render` / `maho-app` / `maho-host` / `maho-mobile` |
| `erd_proto` / `erd_net` / `erd_decode` / `erd_render` / `erd_app` / `erd_host` / `erd_mobile` | `maho_proto` / `maho_net` / `maho_decode` / `maho_render` / `maho_app` / `maho_host` / `maho_mobile` |
| binary `erd-host` / `erd-client` / `erd-discover` | `maho-host` / `maho-client` / `maho-discover` |
| `_erd._tcp` / `_erd._udp` | `_maho-rd._tcp` / `_maho-rd._udp` |
| `com.eclipticrd.shell` / `com.eclipticrd.ios` | `com.projectmaho.mahord` / `com.projectmaho.mahord` |
| `ERD_*` env (`ERD_OUTPUT`, `ERD_AUDIO_MONITOR`, …) | `MAHO_*` |
| `C:\erd` | `C:\maho` (docs/scripts only) |
| `erd-ios` (xcodegen target) + `gen/apple/erd-ios_iOS` | `maho-ios` + `gen/apple/maho-ios_iOS` |
| crate dirs `clients/rust/erd-*` | `clients/rust/maho-*` (git mv) |
| `skills/eclipticrd-remote-control` | `skills/mahord-remote-control` (git mv) |
| systemd unit `erd-host.service` / Windows task `erd-host-run` | `maho-host.service` / `maho-host-run` (docs in repo; actual host changes in deploy phase) |

KEEP UNCHANGED: working dir name `EclipticRD-Rewrite` (local + Omarchy remote), ports 19730/19731/19735, wire protocol v3 bytes, `tauri-shell`/`ios-shell` dir names, MIT license, no logic changes.
FORBIDDEN in sweeps: rewriting inside 40-hex commit SHAs/hashes; touching `target/`, `dist/`, `node_modules/`, untracked build outputs; editing logic; committing.

## Phase 0 — control plane (lead)
1. Verify + commit the 5 tracked-modified pending files (prior-session fix) as an isolated commit.
2. RED captures: C1 old repo exists; C2 residue counts over `git ls-files`.
3. `gh api /orgs/Project-Maho --jq .id`; `gh api -X POST /repos/Indosaram/EclipticRD/transfer -F new_owner_id=<id>`.
4. `gh api -X PATCH /repos/Project-Maho/EclipticRD -f name=MahoRD`.
5. Verify: GET new repo (name/owner/public/MIT), `curl -sI` old URL → 301, `git remote set-url origin https://github.com/Project-Maho/MahoRD.git`.

## Phase 1 — mass-ulw run 1 (`mahord-rebrand-p1-20260912`)
Topology: `sweep-code` ∥ `sweep-docs` (disjoint write scopes) → `rust-gate` (Omarchy) ∥ `ui-gate` (local JS). All `category: quick` (gemini-3.8-flash-high).
- `sweep-code` scope: `clients/rust/**` (rs/toml/json/plist/yml/lock/ts/tsx/mjs). Ends with its own residue gate on scope = 0.
- `sweep-docs` scope: `README.md`, `AGENTS.md`, `walkthrough.md`, `docs/**`, `.omo/**`, `scripts/**`, `skills/**`, `THIRD_PARTY_NOTICES.md`. git mv skills dir.
- `rust-gate`: rsync → `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig:$PKG_CONFIG_PATH cargo check/test/clippy -D warnings --manifest-path clients/rust/Cargo.toml` on indo@100.91.254.71; fix fallout in scope; verbatim outputs.
- `ui-gate`: `bun run`-defined tsc, `bun test src`, `bun test tests` in clients/rust/tauri-shell.
- Lead close-out: residue recount (C2 GREEN=0), diff review, ONE atomic commit `refactor!: rename erd-* crates and all brand identifiers to maho-* (MahoRD)`, push.

## Phase 2 — deploy wave (lead, rollbacks kept)
- Omarchy: build release; write `maho-host.service` unit; `systemctl --user disable --now erd-host`; enable new; verify ports+mDNS.
- Windows (maho-win): cross-compile x86_64-pc-windows-gnu on Omarchy; deploy `C:\maho\releases\<date>-<sha>`; new task `maho-host-run`; migrate host pairing/auth stores; verify.
- Mac: `cargo tauri build --bundles app` (entitlements committed at clients/rust/tauri-shell/macos/entitlements.plist); Developer ID sign; backup; deploy `/Applications/MahoRD.app`; migrate client pairing store from old app-data dir.
- iPhone: rebuild IPA (Mac exception); install only if device connected (physical-device rule); expect 1× PIN re-pair (bundle ID change → Keychain loss).

## Phase 3 — real-surface proof
Mac MahoRD.app: discovery shows Omarchy under `_maho-rd._tcp` → connect → ≥10 frames decoded → agent input action via `maho-client --agent-server` HTTP API; Windows host live check.

## Phase 4 — review
`task` with `subagent_type: omo-senpi-code-reviewer` + `model: opencodex/gpt-6-astra` over the rebrand diff. Criterion-cited blockers fixed (Flash or lead), re-review ≤2 rounds.

## Gates
Omarchy: `cargo check/test/clippy -D warnings --manifest-path clients/rust/Cargo.toml` (FFmpeg7 PKG_CONFIG_PATH). Local JS: tsc 0, `bun test src`, `bun test tests`. Residue: `git ls-files -z | xargs -0 rg -io 'eclipticrd|_erd\.' | wc -l` = 0; `rg -o 'erd[-_]|ERD_'` over tracked = 0.
