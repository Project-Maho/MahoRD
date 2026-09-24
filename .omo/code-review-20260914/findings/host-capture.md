# Lane: host-capture

## Scope reviewed

Current working-tree content was reviewed, including uncommitted working tree changes. No source files were modified.

- `clients/rust/maho-host/src/capture_linux.rs`: 836 lines, read in full.
- `clients/rust/maho-host/src/capture_windows.rs`: 449 lines, read in full.
- `clients/rust/maho-host/src/capture_macos.rs`: 749 lines, read in full.
- `clients/rust/maho-host/src/windows_logic.rs`: 779 lines, read in full.
- `clients/rust/maho-host/src/windows_freshness.rs`: 62 lines, read in full.
- `clients/rust/maho-host/src/native_pipeline.rs`: 714 lines, read in full.
- `clients/rust/maho-host/src/native_stall_tests.rs`: 72 lines, read in full.
- Relevant direct imports and integration callers:
  - `clients/rust/maho-host/src/session.rs`: lines 305-375, 715-770, 1220-1475, 1600-1745, and 2220-2240 (screen capture integration and cursor math).
  - `clients/rust/maho-host/src/encode_windows.rs`: lines 340-360 (NV12 frame length validation).
  - External dependency `dxgi-capture-rs` 1.2.2 source (`src/lib.rs`): lines 450-610 and 990-1110 (`capture_frame_to_surface`, `copy_surface_data`, metadata extraction).

This is an adversarial static code review focusing on safety, lifetime management, stride/pitch math, multi-monitor offset calculations, DPI awareness, and resource release pairing.

## Findings

### [P1] Wayland `zwlr_screencopy_v1` silently drops `Flags::YInvert` because `Flags` event arrives before `BufferDone`
- **Location**: `clients/rust/maho-host/src/capture_linux.rs:691` (secondary site: `clients/rust/maho-host/src/capture_linux.rs:245`)
- **Evidence**:
```rust
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                if let Some(active) = &mut state.active {
                    active.y_inverted = match flags {
                        WEnum::Value(flags) => {
                            flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert)
                        }
                        WEnum::Unknown(_) => false,
                    };
                }
            }
```
```rust
        self.active = Some(ActiveCapture {
            description,
            damage: Vec::new(),
            y_inverted: false,
        });
```
- **Impact**: Under standard Wayland compositors (such as wlroots, Sway, and Hyprland), the compositor emits `buffer`, `flags`, and `buffer_done` events during initial frame negotiation before any `copy` request is sent. In `capture_linux.rs`, `state.active` is `None` until `create_buffer` is called inside `BufferDone`. Consequently, when `Event::Flags` arrives, `if let Some(active) = &mut state.active` fails to match, and the `Flags` event is silently discarded. When `BufferDone` subsequently executes `create_buffer`, `self.active` is instantiated with `y_inverted: false`. On all systems where the compositor uses an OpenGL/GLES backend rendering in bottom-up orientation (standard for wlroots on Mesa drivers), `y_inverted` remains permanently `false`. The image-flipping branch in `finish_capture` never executes, and transmitted video frames appear upside-down on the client.
- **Fix**: Store `y_inverted` directly on `CaptureState`. Reset it to `false` when queuing a capture request, set it in `Event::Flags`, and copy the recorded flag into `ActiveCapture` when `create_buffer` runs.
- **Confidence**: high

### [P1] High-DPI Windows display capture truncates buffer and crashes video encoder due to logical vs physical dimension mismatch
- **Location**: `clients/rust/maho-host/src/capture_windows.rs:149` (secondary sites: `clients/rust/maho-host/src/windows_logic.rs:416`, `clients/rust/maho-host/src/session.rs:1376`)
- **Evidence**:
```rust
        let (bgra, (width, height), metadata) = self
            .manager
            .capture_frame_components_with_metadata()
            .map_err(map_capture_error)?;

        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(CaptureError::InvalidFrame)?;
        if bgra.len() != expected {
            return Err(CaptureError::InvalidFrame);
        }

        let width = u32::try_from(width).map_err(|_| CaptureError::InvalidFrame)?;
        let height = u32::try_from(height).map_err(|_| CaptureError::InvalidFrame)?;
```
```rust
    // Preserve the unrotated physical raster directly from the DXGI duplication mode.
    let pixel_width = dupl.mode_width;
    let pixel_height = dupl.mode_height;

    let raw_log_w = (output.desktop_right - output.desktop_left).unsigned_abs();
    let raw_log_h = (output.desktop_bottom - output.desktop_top).unsigned_abs();
    if raw_log_w == 0 || raw_log_h == 0 {
        return Err("desktop coordinate dimensions must be positive");
    }
    let logical_width = raw_log_w;
    let logical_height = raw_log_h;
```
- **Impact**: In `windows_logic.rs`, `resolve_output_metadata` resolves `pixel_width` and `pixel_height` to the physical raster mode (`dupl.mode_width`, e.g., 3840x1600), which `HostConfig` uses to configure the video encoder (`EncoderConfig`). However, `capture_windows.rs` delegates frame acquisition to `dxgi-capture-rs` 1.2.2, which computes frame dimensions and slices surface memory using `desc.DesktopCoordinates` (logical desktop DIPs, e.g. 3072x1280 at 125% DPI scale, as noted in `capture_windows.rs` line 228 and `windows_logic.rs` line 640). As a result, `capture_windows.rs` returns a frame with `width: 3072, height: 1280` and truncated pixel data (5,898,240 bytes of NV12 instead of 9,216,000 bytes). When submitted to the Windows video encoder, `encode_nv12` fails validation (`expected != nv12.len()`) with `EncodeError::InvalidFrameLength`, crashing the media pipeline on the very first frame on any Windows display with DPI scaling active.
- **Fix**: Determine capture buffer dimensions consistently from the DXGI duplication texture rather than `DesktopCoordinates`. If wrapping `dxgi-capture-rs`, verify that pixel dimensions match `dupl.mode_width` and `dupl.mode_height`, or use the D3D11 staging texture description and pitch directly to extract the full physical raster without truncation.
- **Confidence**: high

### [P1] Multi-adapter Windows display enumeration resets output index, preventing capture of secondary GPU displays
- **Location**: `clients/rust/maho-host/src/capture_windows.rs:259`
- **Evidence**:
```rust
        let Ok(device) = geometry_device(&adapter, Some(&[D3D_FEATURE_LEVEL_9_1])) else {
            continue; // Like the capture manager, skip adapters without a usable device.
        };
        let mut attached_index = 0;
        for output_index in 0.. {
            // SAFETY: adapter is live; enumeration returns owned COM interfaces.
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => output,
                // The dependency treats every EnumOutputs error as end of adapter.
                Err(_) => break,
            };
            // SAFETY: output is live; GetDesc initializes the returned value.
            let desc: DXGI_OUTPUT_DESC = unsafe { output.GetDesc() }.map_err(initialization)?;
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            if attached_index != display_index {
                attached_index += 1;
                continue;
            }
```
- **Impact**: In `selected_output_metadata(display_index)`, `attached_index` is declared and initialized to `0` inside the `for adapter_index in 0..` loop. On multi-GPU systems (standard for laptops with integrated and discrete GPUs, or desktops with multiple graphics cards), each adapter restarts counting attached desktop outputs from 0. If `display_index` is 1 and monitor 1 is connected to adapter 1 (with monitor 0 on adapter 0), adapter 0 has only one output and never matches `attached_index == 1`. The loop then advances to adapter 1, resets `attached_index` to 0, checks output 0 (where `0 != 1`, increments to 1), and then exhausts outputs. The function terminates with `CaptureError::Initialization("No suitable output display was found")`. Any display connected to a secondary adapter is unselectable and uncapturable.
- **Fix**: Move `let mut attached_index = 0;` outside the `for adapter_index in 0..` loop so that desktop-attached display indexing increments monotonically across all DXGI adapters.
- **Confidence**: high

### [P1] Unsupported format in `zwlr_screencopy_v1::Event::Buffer` causes immediate fatal error and protocol violation in version < 3
- **Location**: `clients/rust/maho-host/src/capture_linux.rs:661`
- **Evidence**:
```rust
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let format = match format {
                    WEnum::Value(wl_shm::Format::Argb8888) => wl_shm::Format::Argb8888,
                    WEnum::Value(wl_shm::Format::Xrgb8888) => wl_shm::Format::Xrgb8888,
                    other => {
                        state.result =
                            Some(Err(CaptureError::UnsupportedFormat(format!("{other:?}"))));
                        return;
                    }
                };
                state.offered_buffer = Some(BufferDescription {
                    format,
                    width,
                    height,
                    stride,
                });
                // Protocol versions 1 and 2 have no buffer_done event.
                if frame.version() < 3 {
                    if let Err(error) = state.create_buffer(frame, qh) {
                        state.result = Some(Err(error));
                    }
                }
            }
```
- **Impact**: Under the `zwlr_screencopy_v1` protocol, the compositor may send multiple `buffer` events offering different formats supported by the display hardware or compositor renderer (such as `Xbgr8888`, `Abgr8888`, `Xrgb8888`, and `Argb8888`). By returning `Err(CaptureError::UnsupportedFormat)` on the very first non-matching format, `capture_linux.rs` aborts capture even if a supported format (`Xrgb8888`) is advertised in the next event. Furthermore, when `frame.version() < 3`, `create_buffer` is invoked on every buffer event without waiting, calling `frame.copy()` multiple times on the same `zwlr_screencopy_frame_v1` object. The Wayland screencopy specification states that `copy` must be sent at most once per frame; sending multiple `copy` requests triggers an unrecoverable Wayland protocol error, causing the compositor to terminate the client connection.
- **Fix**: Ignore unsupported formats in `Event::Buffer` without setting `state.result`. Retain the best supported format offer in `state.offered_buffer`. For protocol version < 3, only create and copy the buffer on the first supported format offer. Report `UnsupportedFormat` only in `BufferDone` if `state.offered_buffer` remains `None`.
- **Confidence**: high

### [P1] Remote input injection mixes logical desktop coordinates with physical pixel dimensions, breaking mouse coverage under DPI scaling
- **Location**: `clients/rust/maho-host/src/windows_logic.rs:136` (secondary site: `clients/rust/maho-host/src/session.rs:2227`)
- **Evidence**:
```rust
    let local_x = finite_unit(normalized_x) * target.width.saturating_sub(1) as f64;
    // Protocol convention inverts Y (1.0 - y) for legacy macOS compatibility.
    // Invert it back so (0,0) is top-left on Windows.
    let local_y = (1.0 - finite_unit(normalized_y)) * target.height.saturating_sub(1) as f64;
    let desktop_x = (target.x as f64 + local_x - desktop.x as f64)
        .clamp(0.0, desktop.width.saturating_sub(1) as f64);
    let desktop_y = (target.y as f64 + local_y - desktop.y as f64)
        .clamp(0.0, desktop.height.saturating_sub(1) as f64);
    (
        (desktop_x * ABSOLUTE_AXIS_MAX / desktop.width.saturating_sub(1) as f64).round() as i32,
        (desktop_y * ABSOLUTE_AXIS_MAX / desktop.height.saturating_sub(1) as f64).round() as i32,
    )
```
- **Impact**: In `windows_logic.rs`, `TargetDisplay` documentation specifies physical pixel dimensions, but `session.rs` instantiates `TargetDisplay` with `x: desktop_x, y: desktop_y` (which are logical DIP coordinates from `DXGI_OUTPUT_DESC.DesktopCoordinates`) and `width: pixel_width, height: pixel_height` (which are physical pixels from `dupl.mode_width`, e.g. 3840x1600). The `VirtualDesktop` struct is populated via `GetSystemMetrics(SM_CXVIRTUALSCREEN)`, which operates in logical desktop coordinates (e.g. 3072x1280). When `normalized_x = 1.0`, `local_x` is 3839, which exceeds `desktop.width` (3072). `desktop_x` clamps prematurely at 3071 when `normalized_x` reaches only ~0.80. The rightmost 20% and bottom 20% of the display cannot be reached by remote mouse input, and clicking near the right edge targets the wrong logical position.
- **Fix**: Use consistent coordinate spaces in `TargetDisplay` and `normalize_absolute_pointer`. In `session.rs`, construct `TargetDisplay` using `logical_width` and `logical_height` (matching `desktop_x`, `desktop_y`, and `SM_CXVIRTUALSCREEN`), or scale `GetSystemMetrics` virtual desktop coordinates to physical pixels.
- **Confidence**: high

### [P1] Wayland `wl_buffer` and `wl_shm_pool` server resources leak on resolution changes
- **Location**: `clients/rust/maho-host/src/capture_linux.rs:205`
- **Evidence**:
```rust
        if !is_cached_valid {
            let mut file = tempfile::tempfile()?;
            file.set_len(size)?;
            file.seek(SeekFrom::Start(0))?;

            let shm = self
                .shm
                .as_ref()
                .ok_or(CaptureError::PortalRequired("wl_shm"))?;
            let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
            let buffer = pool.create_buffer(
                0,
                description.width as i32,
                description.height as i32,
                description.stride as i32,
                description.format,
                qh,
                (),
            );
            self.cached_buffer = Some(CachedBuffer {
                file,
                _pool: pool,
                buffer,
                description,
            });
        }
```
- **Impact**: When `is_cached_valid` is `false` (such as on display resolution changes or desktop mode switches), `self.cached_buffer` is overwritten with a newly created `CachedBuffer`. In `wayland-client`, dropping a `WlBuffer` or `WlShmPool` Rust proxy does not send protocol `destroy` requests to the compositor. The compositor continues tracking the previous `wl_buffer` and `wl_shm_pool` objects and keeps the associated shared-memory file descriptor and page mappings alive. Over repeated display mode changes or region captures, this leads to unbounded memory and file descriptor leaks in the Wayland compositor process.
- **Fix**: Explicitly invoke `cached.buffer.destroy()` and `cached._pool.destroy()` on the existing `CachedBuffer` before replacing it or implement `Drop` for `CachedBuffer` that sends the respective destroy requests.
- **Confidence**: high

### [P1] `FrameTimes::take` drops multi-packet PTS and halts video pipeline
- **Location**: `clients/rust/maho-host/src/native_pipeline.rs:356`
- **Evidence**:
```rust
    pub fn submitted(&mut self, capture_at: Instant, encode_started_at: Instant) {
        self.pending
            .insert(self.next_pts, (capture_at, encode_started_at));
        self.next_pts += 1;
    }

    pub fn take(&mut self, pts: i64) -> Option<(Instant, Instant)> {
        self.pending.remove(&pts)
    }
```
- **Impact**: `FrameTimes::take` removes the entry for `pts` from the `pending` map upon the first query. If a video encoder produces more than one output packet with the same input presentation timestamp (for instance, when keyframes emit separate parameter sets and slice NALUs, or when multiple slices are emitted per frame), the second call to `take(pts)` returns `None`. In `session.rs` line 1621, `self.times.take(encoded.pts).ok_or_else(...)` converts `None` into an error (`"encoder output has unknown input PTS ..."`), causing `run_encoder` to immediately terminate the entire streaming session. In addition, if the encoder drops any frame without emitting a packet, the corresponding PTS entry remains in `pending` indefinitely, causing unbounded memory growth over long-running sessions.
- **Fix**: Retain timestamp metadata without deletion until a monotonic PTS horizon advances, or prune entries older than `pts` while allowing multiple packets with the same `pts` to read the timestamp without deletion.
- **Confidence**: high

### [P2] `capture_macos.rs` hardcodes zero desktop origin and main display in `display_info`, breaking multi-monitor coordinates
- **Location**: `clients/rust/maho-host/src/capture_macos.rs:247`
- **Evidence**:
```rust
        pub fn display_info() -> Result<DisplayInfo, CaptureError> {
            let display = CGDisplay::main();
            let logical = display.bounds().size;
            let pixel_width = display.pixels_wide() as u32;
            let pixel_height = display.pixels_high() as u32;
            if pixel_width == 0
                || pixel_height == 0
                || logical.width <= 0.0
                || logical.height <= 0.0
            {
                return Err(CaptureError::NoDisplay);
            }
            let scale = pixel_width as f64 / logical.width;
            Ok(DisplayInfo {
                desktop_x: 0,
                desktop_y: 0,
                logical_width: logical.width.round() as u32,
                logical_height: logical.height.round() as u32,
                pixel_width,
                pixel_height,
                scale_factor_milli: (scale * 1_000.0).round() as u32,
            })
        }
```
- **Impact**: `MacScreenCapture::display_info()` hardcodes `desktop_x: 0` and `desktop_y: 0`, and always queries `CGDisplay::main()`. In multi-monitor arrangements on macOS, displays positioned to the left or above the primary monitor have negative desktop coordinates, and non-primary displays have non-zero origins in global CoreGraphics space (`display.bounds().origin`). If a non-primary display is captured, `display_info` returns the primary display's dimensions and reports (0, 0) as its origin, corrupting mouse event normalization and cursor tracking across monitors.
- **Fix**: Read `let origin = display.bounds().origin;` and assign `desktop_x: origin.x.round() as i32` and `desktop_y: origin.y.round() as i32`. Allow `display_info` to accept a target display ID instead of unconditionally defaulting to `CGDisplay::main()`.
- **Confidence**: high

### [P2] `MoveRect` silently dropped in `capture_windows.rs` if source point coordinates are negative
- **Location**: `clients/rust/maho-host/src/capture_windows.rs:174`
- **Evidence**:
```rust
                Some(MoveRect {
                    source_x: u32::try_from(region.source_point.0).ok()?,
                    source_y: u32::try_from(region.source_point.1).ok()?,
                    destination,
                })
```
- **Impact**: In `capture_windows.rs`, `MoveRect` coordinates are converted via `u32::try_from`. If a window is moved from a boundary where `source_point.0` or `source_point.1` is negative (such as an off-screen region or multi-monitor arrangement), `u32::try_from` returns `Err` and the `?` operator silently drops the entire `MoveRect`. The client will not apply the copy operation for this region, leaving stale or torn pixel artifacts on screen unless a subsequent full redraw occurs.
- **Fix**: Retain signed integer representation for `source_x` and `source_y` matching the Windows `POINT` type, or clip the move rectangle to valid source coordinates before converting to unsigned integers.
- **Confidence**: high

### [P2] `capture_linux.rs` performs synchronous file I/O on unlinked tempfile instead of memory-mapping
- **Location**: `clients/rust/maho-host/src/capture_linux.rs:271`
- **Evidence**:
```rust
        if !active.y_inverted && description.stride as usize == row_bytes {
            cached.file.read_exact(&mut bgra)?;
        } else {
            let source_len = description.stride as usize * description.height as usize;
            if self.raw_shm_buf.len() < source_len {
                self.raw_shm_buf.resize(source_len, 0);
            }
            cached
                .file
                .read_exact(&mut self.raw_shm_buf[..source_len])?;
```
- **Impact**: On every frame captured, `finish_capture` seeks to offset 0 in `cached.file` (an unlinked file created in `/tmp` via `tempfile::tempfile()`) and executes `read_exact` through the VFS/page cache layer. For a 4K frame at 60 FPS, this translates to reading ~2 GB/s of data via kernel file read syscalls and copying memory multiple times. If `/tmp` resides on a physical filesystem rather than tmpfs, this produces heavy disk I/O and SSD write wear.
- **Fix**: Use `rustix::fs::memfd_create` to allocate purely in-memory anonymous shared memory buffers, and memory-map the fd with `mmap` so frame pixels can be read directly from memory rather than via `read_exact` syscalls.
- **Confidence**: high

## Non-findings checked
- `CVPixelBufferLockBaseAddress` and `CVPixelBufferUnlockBaseAddress` in `capture_macos.rs:197-212` are strictly paired around raw slice copy, ensuring safe pointer access.
- `MacScreenCapture` stop sequence in `capture_macos.rs:440-475` uses reference counting to guarantee callbacks own teardown resources without double-stopping or leaking upon waiter timeout.
- `bgra` length validation in `capture_windows.rs:154-159` uses checked multiplication to avoid integer overflow before verifying buffer length against dimensions.
- `capture_linux.rs:480-520` handles `poll()` signals with proper check for `EINTR`, avoiding premature capture abortion during signal interrupts.
- `LatestAudio` slot synchronization in `native_pipeline.rs:305-330` guarantees latest-block semantics and bounds audio latency to `MAX_AGE` (100ms) without queue drift.
- `Handoff::reselect` in `native_pipeline.rs:125-140` coalesces `force_keyframe` bitwise so bitrate adjustment reopens do not drop pending keyframe intent.
- `windows_freshness.rs:16-29` correctly preserves the original `captured_at` instant across repeat frames while tracking distinct publication ages.
- `windows_logic.rs:408-415` correctly detects and rejects non-identity DXGI display rotations (e.g. 90/180/270 degrees) before attempting un-rotated raster streaming.
