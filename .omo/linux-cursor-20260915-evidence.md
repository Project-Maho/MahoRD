# Linux Cursor Fix Evidence & Reproducibility Report

Task ID: st_01a0a3a3

## 1. Exact Isolated Remote Path
- Remote host: `indo@100.91.254.71` (Omarchy)
- Isolated snapshot path: `/home/indo/projects/erd-isolated-st_01a0a3a3`
- Workspace root: `/home/indo/projects/erd-isolated-st_01a0a3a3/clients/rust`
- Commit base: `HEAD` (`030422ce3e5a93a02a6841eeb35d987acd0a7a52`)
- Source verification: Only `clients/rust/maho-host/src/session.rs` contains edits relative to HEAD (`git status` and `git diff` show exactly one modified file).

## 2. Reproducible Remote Commands

### Environment Setup
```bash
export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig
export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib
cd /home/indo/projects/erd-isolated-st_01a0a3a3/clients/rust
```

### Formatting Verification
```bash
cargo fmt --check -- maho-host/src/session.rs
```
Result: Clean exit (code 0), zero formatting violations.

### Scoped Unit Tests (Regression Suite)
```bash
cargo test -p maho-host --lib regression_multi_monitor_output_geometry_matches_desktop_topology
cargo test -p maho-host --lib deterministic_exact_target_selection_and_fallback_precedence
```
Result: Both tests passed (1 passed each, code 0).

### Full Host Unit Test Suite
```bash
cargo test -p maho-host --lib
```
Result: 127 passed, 0 failed.

### Integration Test Suite
```bash
cargo test -p maho-host --test linux_audio_selection --test pairing_isolation
```
Result: 8 passed (6 audio selection, 2 pairing isolation), 0 failed.

## 3. Manual Real-Path Live Probe Evidence
Earlier manual real-path execution of `probe_linux_input_geometry(Some("HDMI-A-2"))` directly against the live Hyprland instance on Omarchy (`HYPRLAND_INSTANCE_SIGNATURE=efb50993780079460b0cbed1363e2166a2de1d9f_1789367143_909438767`):
```text
Live real-path probed geometry: OutputGeometry {
    x: 2560,
    y: 0,
    width: 3840,
    height: 1600,
    desktop_x: 0,
    desktop_y: 0,
    desktop_width: 6400,
    desktop_height: 1600
}
```
Normalized mapping of (1.0, 1.0) maps to pixel `(6399, 0)`, aligning with the compositor global layout across HEADLESS-1 (0,0) and HDMI-A-2 (2560,0).

## 4. Live QA Blocker Details
- Active daemon process: PID 1938 (`releases/20260914-release/maho-host`)
- Status: At 11:38:17 UTC, the running daemon encountered a Wayland protocol error:
  `Protocol error 4294967295 on object zwlr_screencopy_manager_v1@4: invalid output`
- Consequence: Port 19730 has an unserviced accept queue (`Recv-Q: 1`). Any incoming CLI TLS connections block waiting for the host to complete the TLS handshake (`WouldBlock`).
- Per task constraints ("No commits, deployment, service restart, or GUI automation"), no systemd user service restart (`systemctl --user restart maho-host.service`) was performed. Live end-to-end client verification against the system daemon remains gated on service restart during an authorized maintenance window.
