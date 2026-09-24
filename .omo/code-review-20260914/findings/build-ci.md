# Lane: build-ci
## Scope reviewed
- `Cargo.toml`: 3 lines
- `clients/rust/rust-toolchain.toml`: 3 lines
- `clients/rust/Cargo.toml`: 37 lines
- `clients/rust/maho-proto/Cargo.toml`: 9 lines
- `clients/rust/maho-net/Cargo.toml`: 33 lines
- `clients/rust/maho-decode/Cargo.toml`: 24 lines
- `clients/rust/maho-render/Cargo.toml`: 19 lines
- `clients/rust/maho-app/Cargo.toml`: 44 lines
- `clients/rust/maho-host/Cargo.toml`: 89 lines
- `clients/rust/maho-mobile/Cargo.toml`: 25 lines
- `clients/rust/ios-shell/Cargo.toml`: 47 lines
- `clients/rust/tauri-shell/Cargo.toml`: 37 lines
- `clients/rust/tauri-shell/package.json`: 33 lines
- `clients/rust/tauri-shell/vite.config.ts`: 11 lines
- `clients/rust/tauri-shell/tauri.conf.json`: 35 lines
- `.github/workflows/rust-client.yml`: 56 lines
- `.github/workflows/rust-matrix.yml`: 134 lines

## Findings

### [P1] CI workflows completely omit clippy, rustfmt, frontend test, and TypeScript checks
- **Location**: `.github/workflows/rust-client.yml:18` (and `.github/workflows/rust-matrix.yml:85`)
- **Evidence**:
`.github/workflows/rust-client.yml:18-21`:
```yaml
    steps:
      - uses: actions/checkout@v4
      - run: cargo build --workspace
      - run: cargo test --workspace
```
`.github/workflows/rust-matrix.yml:85-92`:
```yaml
      - name: Build Workspace Binaries
        run: cargo build --workspace --release --target ${{ matrix.target }}

      - name: Run Workspace Tests
        run: |
          # Test workspace where feasible on the host platform
          cargo test --workspace
```
- **Impact**: Code formatting drift (`cargo fmt --all -- --check` already fails with dozens of diffs in `maho-net`, `maho-proto`, `maho-render`, and `tauri-shell`), clippy lint regressions, and frontend type/test breakages (`bun test src`, `tsc -b`) pass CI undetected. Any PR breaking web UI or introducing Rust linter regressions gets merged without blocker signals.
- **Fix**: Add CI steps or dedicated jobs for `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and frontend verification (`bun install --frozen-lockfile`, `bun run typecheck`, and `bun test src` in `clients/rust/tauri-shell`).
- **Confidence**: high

### [P1] CI build matrix for Linux packages `maho-client` without GTK3 / WebKitGTK runtime dependencies needed by `tauri-shell`
- **Location**: `.github/workflows/rust-matrix.yml:25` (and `.github/workflows/rust-matrix.yml:45`)
- **Evidence**:
`.github/workflows/rust-matrix.yml:25-30`:
```yaml
          - name: Linux
            os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            features: default
            binary_name: maho-client
            artifact_name: mahord-linux-x86_64
```
`.github/workflows/rust-matrix.yml:45-60`:
```yaml
      - name: Install Linux build dependencies (FFmpeg / Clang / Audio)
        if: matrix.os == 'ubuntu-latest'
        run: |
          sudo apt-get update
          sudo apt-get install -y \
            clang \
            libasound2-dev \
            libavcodec-dev \
            libavformat-dev \
            libavutil-dev \
            libclang-dev \
            libswresample-dev \
            libswscale-dev \
            libudev-dev \
            libva-dev \
            libwayland-dev \
            libx264-dev \
            pkg-config
```
- **Impact**: Step `cargo build --workspace --release --target ${{ matrix.target }}` attempts to compile `tauri-shell`, which depends on `tauri` with feature `"wry"` requiring `libwebkit2gtk-4.1-dev` and `libgtk-3-dev` on Linux. Furthermore, step `Stage Artifacts` attempts to package `tauri-shell`, but only `maho-client` CLI and `maho-host` are built/installed, leaving the desktop UI unbuilt and unbundled in CI Linux releases.
- **Fix**: Install required GTK3 and WebKit2GTK system libraries (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libsoup-3.0-dev`, `libjavascriptcoregtk-4.1-dev`) in the Linux CI job, and specify the intended artifact list consistently.
- **Confidence**: high

### [P2] Incompatible `rand` version specifications causing duplicate SemVer versions in lockfile
- **Location**: `clients/rust/Cargo.toml:36` and `clients/rust/maho-host/Cargo.toml:30`
- **Evidence**:
`clients/rust/Cargo.toml:36`:
```toml
rand = "0.8"
```
`clients/rust/maho-host/Cargo.toml:30`:
```toml
rand = "0.9"
```
- **Impact**: The workspace pulls both `rand 0.8.8` and `rand 0.9.5` into the dependency graph. Types and traits from `rand::RngCore`, `rand::distributions`, and RNG implementations are incompatible across major versions. If random number generators or seeded state are passed between `maho-host` and `maho-app` or other workspace crates, compilation fails or requires duplicate transitive crates (`rand_core 0.6` vs `rand_core 0.9`).
- **Fix**: Standardize `rand` across the workspace by inheriting `rand.workspace = true` in `maho-host/Cargo.toml`, upgrading the workspace specification if `rand 0.9` APIs are required.
- **Confidence**: high

### [P2] Duplicate / unused dependencies declared in workspace manifests
- **Location**: `clients/rust/Cargo.toml:27` and `clients/rust/maho-net/Cargo.toml:15`
- **Evidence**:
`clients/rust/Cargo.toml:27-28`:
```toml
wgpu = "26"
winit = "0.30"
```
`clients/rust/maho-net/Cargo.toml:14-15`:
```toml
openssl.workspace = true
openssl-sys = { version = "0.9", features = ["vendored"] }
```
- **Impact**: `wgpu` and `winit` are declared in `[workspace.dependencies]` but never used by any crate in the workspace (the renderer uses WebGPU/WebGL via the webview and native rendering is handled outside these crates). In `maho-net/Cargo.toml`, `openssl.workspace = true` (which already specifies `features = ["vendored"]` at `clients/rust/Cargo.toml:22`) is declared alongside an explicit direct dependency on `openssl-sys`, causing redundant manifest noise and potential feature-set mismatch.
- **Fix**: Remove unused `wgpu` and `winit` entries from `clients/rust/Cargo.toml`, and remove redundant `openssl-sys` dependency in `maho-net/Cargo.toml`.
- **Confidence**: high

### [P2] Workspace root trap: repo root `Cargo.toml` points to `crates/spike-sck` instead of Rust client workspace
- **Location**: `Cargo.toml:1`
- **Evidence**:
`Cargo.toml:1-3`:
```toml
[workspace]
resolver = "2"
members = ["crates/spike-sck"]
```
- **Impact**: Running standard development and CI commands (`cargo build`, `cargo test`, `cargo clippy`, `cargo check`) from the repo root executes only the `spike-sck` spike crate instead of the actual `clients/rust` workspace. Any developer or automation tool running `cargo` without `--manifest-path clients/rust/Cargo.toml` or `cd clients/rust` operates on the wrong workspace, creating a silent false-green verification trap.
- **Fix**: Add `clients/rust` crates or a virtual workspace definition, or document the multi-workspace layout in repository root automation scripts with explicit `--manifest-path clients/rust/Cargo.toml`.
- **Confidence**: high

### [P2] Missing `beforeBuildCommand` in `tauri.conf.json` causing release packaging failures if frontend is not pre-built
- **Location**: `clients/rust/tauri-shell/tauri.conf.json:6`
- **Evidence**:
`clients/rust/tauri-shell/tauri.conf.json:6-9`:
```json
  "build": {
    "frontendDist": "./dist",
    "devUrl": "http://localhost:1420"
  },
```
- **Impact**: When running `tauri build` or `cargo tauri build`, Tauri expects either `frontendDist` to already exist or `beforeBuildCommand` to trigger the frontend build (e.g. `"bun run build"`). Because `beforeBuildCommand` is omitted, clean builds from scratch (such as in CI or new clone) fail during asset embedding at `tauri::generate_context!()` if `dist/` has not been manually built ahead of time.
- **Fix**: Add `"beforeBuildCommand": "bun run build"` to `build` in `clients/rust/tauri-shell/tauri.conf.json`.
- **Confidence**: high

## Non-findings checked
- Confirmed `clients/rust/rust-toolchain.toml` specifies `channel = "stable"` with `profile = "minimal"` matching compiler expectations.
- Confirmed `clients/rust/tauri-shell/src-tauri/src/lib.rs` and `src-tauri/src/main.rs` are properly registered as library and binary targets in `clients/rust/tauri-shell/Cargo.toml`.
- Confirmed `clients/rust/tauri-shell/macos/entitlements.plist` exists and matches the `"macOS": { "entitlements": "macos/entitlements.plist" }` path in `tauri.conf.json`.
- Confirmed `clients/rust/tauri-shell/vite.config.ts` outputs to `dist`, aligning with `frontendDist: "./dist"` in `tauri.conf.json`.
- Confirmed `clients/rust/tauri-shell/package.json` scripts (`build`, `test`, `typecheck`) are functioning and pass locally (`bun test src` 95 tests pass, `tsc -b` succeeds).
- Confirmed platform-specific conditional dependencies in `maho-net`, `maho-host`, `maho-app`, `maho-mobile`, and `ios-shell` use proper target cfg gates (`cfg(any(target_os = "macos", target_os = "ios"))`, `cfg(target_os = "windows")`, `cfg(target_os = "linux")`).
- Confirmed `clients/rust/Cargo.lock` is version-controlled and matches workspace package dependency graphs.
