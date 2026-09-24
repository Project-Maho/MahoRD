# Ultrawork Notepad — shadcn/ui migration of tauri-shell, all implementation by gemini-3.8-flash-high
Started: 2026-09-11T16:10:21.485Z
Goal: registered via create_goal (HEAVY tier)
Plan file: .omo/plans/shadcn-migration-flash.md
Source plan: docs/shadcn-migration-plan-20260911.md

## Skills surveyed
- mass-ulw + references/planning.md — READ IN FULL; this session executes its dag workflow.
- bun-1-4 — READ; eval kernel is Bun 1.4, Bun.WebView is my QA channel, Bun.spawn for shell.
- visual-qa — to read before my final real-surface verification (screenshot evidence discipline).
- frontend / impeccable / programming — named in child prompts via load_skills, not read here (children execute them).
- git-master — commit discipline for per-phase atomic commits.
- memory-discipline — durable facts recorded at the end.

## Tier
HEAVY. Justification: new frontend toolchain + layer (React/Vite/Tailwind), refactor crossing the entire client UI, user demanded lead-performed final verification.

## Routing decision (load-bearing)
~/.omo/omo.json categories -> models:
  quick              = "mahoquot/gemini-3.8-flash-high"   (SINGLE model, no fallback)  <= ONLY safe category
  visual-engineering = [flash-high, zai/glm-5.3-flash]    (can fall back off Flash)
  unspecified-low    = [flash-high, flash-high, glm-5.3-flash]
  deep / unspecified-high / ultrabrain = opencodex/gpt-6-astra  (FORBIDDEN this run)
  architect / artistry = include gpt-6-astra              (FORBIDDEN this run)
=> EVERY implementation node is category "quick". This satisfies the user's "3.8 flash high only"
   constraint AND the mass-ulw split-first doctrine (many small quick lanes > one big node).

## Deliberate deviation from docs/shadcn-migration-plan-20260911.md P0 step 5
tauri.conf.json paths resolve relative to tauri-shell/src-tauri/ (evidence: "../icons/icon.png"
resolves to tauri-shell/icons, "../ui" resolves to tauri-shell/ui). So frontendDist becomes
"../dist" == tauri-shell/dist. beforeBuildCommand/beforeDevCommand are NOT added: we build with
`cargo build -p tauri-shell`, which never invokes them, so they would be decoration with an
ambiguous cwd. Consequence recorded: `bun run build` MUST run before any cargo build because
tauri-build embeds frontendDist at compile time.

## Plan (exhaustively detailed)
Phase A (dag run 1) — foundation, 7 nodes:
  A1 scaffold      package.json, vite.config.ts, tsconfig*.json, index.html, src/main.tsx, src/app/App.tsx, components.json, .gitignore
  A2 tokens        src/styles/globals.css token bridge + import line in src/main.tsx        (dependsOn A1)
  A3 lib-library   ui/library.js        -> src/lib/library.ts    + library.test.ts          (dependsOn A1)
  A4 lib-connection ui/connection-state.js -> src/lib/connection.ts + connection.test.ts    (dependsOn A1)
  A5 lib-overlay   ui/session-overlay.js -> src/lib/overlay.ts   + overlay.test.ts          (dependsOn A1)
  A6 lib-ipc       src/lib/ipc.ts typed boundary over all 22 #[tauri::command] + ipc.test.ts (dependsOn A1)
  A7 tauri-conf    tauri.conf.json frontendDist -> ../dist, devUrl                          (dependsOn A1)
  A8 verify        bun install && bun run build && bun test && cargo check -p tauri-shell   (dependsOn A1..A7)
Phase B (dag run 2) — shadcn primitives + launcher features.
Phase C (dag run 3) — session surface (WebGL hot path, verbatim port).
Phase D (dag run 4) — cleanup: delete ui/, withGlobalTauri false, move capabilities.
Final — LEAD ONLY: Bun.WebView real-surface QA at 1280x800 + 390px, cargo build, app launch, cleanup receipts.

## Success criteria + QA scenarios
Copied from the registered goal (criteria 1-7). Each needs RED captured before GREEN and an
evidence artifact path recorded below.

## Now
Phase A dag definition + start.

## Todo
See senpi todo list (mirrors this plan).

## Findings
- tauri-shell has NO package.json, NO bundler; ui/ is served raw (frontendDist "../ui").
- ui/index.html 1,524 lines; inline <script> starts line 171 (~1,350 lines of app logic).
- ui/*.js are UMD (module.exports + window global); tests use createRequire.
- 22 #[tauri::command] in src-tauri/src/lib.rs (verified count).
- Dynamically dispatched commands a grep-port would lose: start_host/stop_host (index.html:860),
  audio_status/list_audio_devices/set_audio_volume/set_audio_muted/set_audio_device via audioCommand().
- performance.test.mjs regex-extracts the inline <script> from index.html and vm-evaluates it;
  session-overlay.test.mjs asserts on index.html markup strings. Both die on migration (540 lines).
- eval js kernel cwd was deleted -> process.cwd()/Bun.$ broken; shell goes through
  Bun.spawn(["/bin/sh","-lc",cmd],{cwd:ROOT}) via globalThis.sh. process.chdir() is unsupported in workers.

## Learnings
- Always read ~/.omo/omo.json categories before promising a model constraint: only a category whose
  value is a single model string guarantees that model.

## Evidence ledger
(appended as captured)
