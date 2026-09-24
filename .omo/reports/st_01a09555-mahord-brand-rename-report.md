# MahoRD Brand Rename Report — st_01a09555

Repo: /Volumes/T9-Mac/project/EclipticRD-Rewrite. Scope: README.md, AGENTS.md, walkthrough.md, THIRD_PARTY_NOTICES.md, docs/**, .omo/**, scripts/**, skills/**, .github/** (tracked files only; clients/rust untouched — owned by another node).

## (a) Residue gate — zero matches

Canonical gate over in-scope tracked files:

    git ls-files -- README.md AGENTS.md walkthrough.md THIRD_PARTY_NOTICES.md docs .omo scripts skills \
      | xargs rg -n 'erd[-_]|ERD_|_erd\.|eclipticrd|EclipticRD'

Raw output: 62 lines, ALL of them `EclipticRD-Rewrite` path references, which the task's own KEEP rule requires to stay unchanged (the directory name EclipticRD-Rewrite in all path references). Filtered for the KEEP exception:

    <same command> | grep -v 'EclipticRD-Rewrite'

Result: EMPTY (rg exit 1, zero matches). Proof of zero genuine old-brand residue in scope.

## (b) git status --porcelain summary

    105 M   (content modified in place)
    54  R   (pure rename, content unchanged)
    98  RM  (rename + content modification)
    118 ??  (pre-existing untracked, untouched by this task)

Renames (git mv, staged): skills/eclipticrd-remote-control -> skills/mahord-remote-control; two evidence dirs .omo/evidence/ulw/{<session-uuid>,session}/G001-eclipticrd-rust-tauri-p0-p7-erd-prot -> G001-mahord-rust-tauri-p0-p7-maho-prot (matching the goalId text, also rewritten).

## (c) EclipticRD-Rewrite path references INTACT

`rg -l 'EclipticRD-Rewrite'` over in-scope tracked files (18 files):
AGENTS.md; docs/ai-agent-remote-desktop-review.md; docs/ios-device-implementation-20260909.md; docs/rust-migration-review.md; docs/evidence/gate-test.log; docs/evidence/gate-clippy.log; docs/evidence/gate-bench.log; docs/evidence/gate-xctest.log; .omo/evidence/ulw/01a05807-.../G001-mahord-rust-tauri-p0-p7-maho-prot/a0/{gate-clippy.log,gate-bench.log,gate-test.log,gate-xctest.log,rust-migration-review.md}; .omo/evidence/ulw/session/G001-mahord-rust-tauri-p0-p7-maho-prot/a0/{gate-clippy.log,gate-bench.log,gate-test.log,gate-xctest.log,rust-migration-review.md}

Samples verified: AGENTS.md `~/projects/EclipticRD-Rewrite` (local + remote paths unchanged), docs/ios-device-implementation-20260909.md `/Volumes/T9-Mac/project/EclipticRD-Rewrite/...` unchanged. Ports 19730/19731/19735: unchanged (README.md 5 hits, docs/protocol-v3.md 2 hits).

## Mapping applied

Canonical substitution (exact command from task) + follow-up passes for tokens the gate caught beyond the mapping:
- `erd-b1`/`erd-p1.*` identities, `erd-ffmpeg7`, `/tmp/erd-{interop,bench,bench-omarchy}`, `~/erd`, `/Users/admin/erd`, `erd-{lan,performance,release,pairing}-*` builder dirs, `erd-pairing-qa-host`, `erd-r3-qa-`, `erd-agent-first-qa`, `erd-headless-client`, `erd-prot`, `X-ERD-Token`/`x_erd_token`, ANSI-adjacent `erd_host`/`erd_client`/`erd_discover`, `ERDConstants`/`ERDCrypto`/`ERDIdentity` -> Maho*/maho-*, standalone prose `ERD` -> `MahoRD`, `grep -E 'erd|...'` -> `'maho|...'`, `ERD isolated R3 QA` hostname -> `MahoRD isolated R3 QA`, truncated `erd-...` -> `maho-...`.

## (d) Ambiguous leftovers, with reasons

1. HKDF salt/info protocol constants left UNCHANGED: `erd/bootstrap/v3`, `erd/tls-psk`, `erd/udp-ikm/v3`, `erd/udp-c2h/v3`, `erd/udp-h2c/v3` (incl. `/nonce` suffix forms), `erd/signaling/v3`, `erd/topic`, `erd/payload-key`, and the ntfy topic prefix `erd3-` (docs/protocol-v3.md, .omo/drafts/tauri-client.md, .omo/plans/tauri-client.md, docs/ai-agent-remote-desktop-audit.md). Reason: these are wire/key-derivation constants whose canonical values live in out-of-scope clients/rust code owned by another node; the MAPPING does not cover them and the residue-gate regex deliberately excludes slash forms. Renaming docs unilaterally would desync the spec from the implementation (interop risk). They should follow whenever the code side renames.
2. Physical external paths renamed in text/scripts but not renameable from here: `/home/indo/maho-ffmpeg7` (was erd-ffmpeg7) on the remote builder VM, `~/maho` (was `~/erd`) inside the tart VM, `C:\maho` (was `C:\erd`, per mapping #8). Reason: brand path constants in scripts/docs renamed mechanically; the physical directories on remote hosts were not (and cannot be) renamed by this task. Follow-up needed on VMs/hosts or the affected scripts (bench-latency.sh, bench-omarchy.sh, run-host-omarchy.sh, with-ffmpeg7.sh, build-ffmpeg-omarchy.sh, vm-interop.sh) will 404 their paths.
3. Third-party strings untouched (false positives, not brand): `erased-serde`, `serde*` crates (docs/dependency-licenses.md), `/dev/dri/renderD128` (Linux device node), substrings inside `ObserverDebug`/`ImageMounterDeviceLocked`/`xcodebuild`.
4. Untracked files/dirs excluded by construction: 118 pre-existing untracked entries (`.debug-journal.md`, `.omo/*-2026090*/` etc.) — instructions forbid touching untracked files; the gate iterates `git ls-files` (tracked only).

## Not touched / verified

- clients/rust/** untouched; no target/, dist/, node_modules/ edits; no compiler runs; no 40-hex SHA strings altered ('erd'/'ERD' cannot occur inside hex); no git add/commit/push.
- Zero `com.eclipticrd`, zero `_erd._tcp/_udp`; `com.projectmaho.mahord` present (4 docs); `_maho-rd._tcp.local.` present; `MAHO_` env prefix present; old skill path `eclipticrd-remote-control` zero refs, new path referenced in 4 files.
