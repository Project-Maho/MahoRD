# Lane: host-io

## Scope reviewed
- `clients/rust/maho-host/src/inject_linux.rs` (688 lines)
- `clients/rust/maho-host/src/inject_windows.rs` (294 lines)
- `clients/rust/maho-host/src/inject_macos.rs` (688 lines)
- `clients/rust/maho-host/src/audio_linux.rs` (569 lines)
- `clients/rust/maho-host/src/audio_windows.rs` (179 lines)
- `clients/rust/maho-host/src/clipboard_linux.rs` (297 lines)
- `clients/rust/maho-host/src/clipboard_windows.rs` (171 lines)
- Direct dependencies reviewed for correctness: `clients/rust/maho-host/src/windows_logic.rs` (660 lines), `clients/rust/maho-proto/src/input.rs` (305 lines)

## Findings

### [P0] Synchronous clipboard child process execution without timeout can deadlock host session thread
- **Location**: `clients/rust/maho-host/src/clipboard_linux.rs:235-254`
- **Evidence**:
```rust
fn read_command_bounded(command: &mut Command, limit: usize) -> Result<Vec<u8>, ClipboardError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut bytes = Vec::with_capacity(limit.min(1024));
    child
        .stdout
        .take()
        .ok_or_else(|| ClipboardError::Command("clipboard stdout was unavailable".into()))?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    let output = child.wait_with_output()?;
    if bytes.len() > limit {
        return Err(ClipboardError::TooLarge);
    }
    if !output.status.success() {
        return Err(ClipboardError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(bytes)
}
```
- **Impact**: In Wayland and X11, retrieving the clipboard selection requires an IPC transaction with the application currently owning the selection (`wl-paste` or `xclip` asks the selection owner over the Wayland/X11 socket). If that client application is suspended (e.g. stopped in GDB, unresponsive, hung on I/O, or frozen), `wl-paste` or `xclip` blocks indefinitely waiting for the selection data. Because `read_command_bounded` spawns the child and executes synchronous blocking `read_to_end` and `wait_with_output` without a deadline, and because `LinuxClipboard::poll` is executed on the main host session thread every 500 ms, the entire host session thread deadlocks permanently. Video streaming, audio capture, TCP heartbeats, and peer input processing all halt until the process is forcefully killed.
- **Fix**: Run `wl-paste` and `xclip` invocations through a cancellable deadline pattern (like `LinuxAudioCapture::query_source` using `poll()` with bounded timeout) or spawn the clipboard poll on a dedicated background worker thread with a bounded timeout (e.g. 500 ms) and kill/reap the child if the deadline expires.
- **Confidence**: high

### [P1] macOS input rate limiter drops KeyUp, MouseUp, and Reset events, causing permanently stuck keys, buttons, and drag states
- **Location**: `clients/rust/maho-host/src/inject_macos.rs:99-104`
- **Evidence**:
```rust
        if !self.allow_event() {
            return Err(InputError::RateLimited);
        }
        if let Some(cg_event) = self.create_event(event)? {
            cg_event.post(core_graphics::event::CGEventTapLocation::HID);
        }
```
- **Impact**: The token bucket rate limiter (`BURST_CAPACITY = 400.0`, `MAX_EVENTS_PER_SECOND = 200.0`) indiscriminately throttles all incoming input events when tokens are depleted. High-frequency gaming mice (500–1000 Hz) and rapid trackpad panning easily exhaust 200 events/second. When the bucket is empty, `inject()` immediately returns `Err(InputError::RateLimited)` without executing `create_event`. If a `KeyUp`, `LeftMouseUp`, `RightMouseUp`, `MiddleMouseUp`, `PenUp`, or `Reset` event arrives during a rate-limited burst, the release event is dropped. Because `create_event` is never reached, the release is never posted to macOS HID and is never removed from `active_keys` or `active_mouse_buttons`. As a result, the affected key repeats indefinitely via macOS typematic repeat, and the mouse remains locked in a drag state until a physical keystroke or click occurs on the host.
- **Fix**: Exclude terminal release events (`KeyUp`, `LeftMouseUp`, `RightMouseUp`, `MiddleMouseUp`, `PenUp`, and `Reset`) from the rate limiter check, or consume tokens only for continuous motion events (`MouseMove`, `LeftMouseDragged`, `RightMouseDragged`, `ScrollWheel`). Never throttle release or reset events.
- **Confidence**: high

### [P1] Windows injector ignores right-side modifier keys and CapsLock, producing duplicate keypresses and breaking AltGr
- **Location**: `clients/rust/maho-host/src/inject_windows.rs:145-155` and `clients/rust/maho-host/src/inject_windows.rs:198-212`
- **Evidence**:
```rust
                if let Some(vk) = macos_keycode_to_vk(event.key_code) {
                    let is_mod_key = matches!(
                        VIRTUAL_KEY(vk),
                        VK_LSHIFT | VK_LCONTROL | VK_LMENU | VK_LWIN
                    );
                    if !is_mod_key {
                        inputs.extend(modifier_inputs(self.modifiers, event.modifiers));
                        self.modifiers = event.modifiers;
                    }
                    inputs.push(key_input(vk, is_up));
                }
```
and:
```rust
fn modifier_inputs(previous: Modifiers, current: Modifiers) -> Vec<INPUT> {
    [
        (Modifiers::SHIFT, VK_LSHIFT.0),
        (Modifiers::CONTROL, VK_LCONTROL.0),
        (Modifiers::OPTION, VK_LMENU.0),
        (Modifiers::COMMAND, VK_LWIN.0),
    ]
    .into_iter()
    .filter_map(|(modifier, vk)| {
        let was_down = previous.contains(modifier);
        let is_down = current.contains(modifier);
        (was_down != is_down).then(|| key_input(vk, !is_down))
    })
    .collect()
}
```
- **Impact**: `is_mod_key` only checks the 4 left modifier keys (`VK_LSHIFT`, `VK_LCONTROL`, `VK_LMENU`, `VK_LWIN`). When a remote user presses Right Shift (`VK_RSHIFT` = 0xA1), Right Ctrl (`VK_RCONTROL` = 0xA3), Right Alt / AltGr (`VK_RMENU` = 0xA5), Right Win (`VK_RWIN` = 0x5C), or CapsLock (`VK_CAPITAL` = 0x14), `is_mod_key` evaluates to `false`. Because `!is_mod_key` is true, the injector queries `modifier_inputs`, which synthesizes an extra left-side modifier press (`VK_LMENU`, `VK_LSHIFT`, etc.), followed by the right-side key press. On Windows, injecting Left Alt simultaneously with Right Alt destroys AltGr behavior (used on European layouts to type `@`, `€`, `\`, `[`, `{`, `~`), triggering standard Alt menu bar activation instead. In addition, `modifier_inputs` omits `Modifiers::CAPS_LOCK`, preventing CapsLock flag synchronization.
- **Fix**: Update `is_mod_key` to match all modifier keys:
`matches!(VIRTUAL_KEY(vk), VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN | VK_CAPITAL)`.
In `modifier_inputs`, include `(Modifiers::CAPS_LOCK, VK_CAPITAL.0)`.
- **Confidence**: high

### [P1] Windows clipboard size validation uses HGLOBAL allocation size instead of string length, discarding valid clipboard text
- **Location**: `clients/rust/maho-host/src/clipboard_windows.rs:115-121`
- **Evidence**:
```rust
        // Avoid allocating an arbitrarily large UTF-16 selection. Four KiB of
        // UTF-8 cannot require more than 8 KiB plus the UTF-16 terminator.
        let max_utf16_storage = MAX_CLIPBOARD_TEXT_BYTES * 2 + 2;
        if raw::size(CF_UNICODETEXT).is_some_and(|bytes| bytes.get() > max_utf16_storage) {
            drop(clipboard);
            return Ok(None);
        }
```
- **Impact**: `raw::size(CF_UNICODETEXT)` invokes Windows `GlobalSize(hMem)` on the clipboard object. `GlobalSize` returns the total allocated capacity of the underlying heap block allocated by the copying application, not the byte length of the null-terminated UTF-16 string. Major applications—including Microsoft Edge, Chrome, Visual Studio, and Microsoft Office—frequently allocate clipboard global memory in default chunks of 16 KiB, 32 KiB, or 64 KiB, even when copying a tiny 5-character string. Because `bytes.get() > 8194` evaluates to `true` for any such allocation, `WindowsClipboard::poll()` silently drops the text and returns `Ok(None)`. Legitimate clipboard copies on Windows are routinely not synchronized to the remote client.
- **Fix**: Remove the pre-check against `raw::size(CF_UNICODETEXT)` or increase its threshold to an upper safety bound (e.g. 1 MiB). Rely on `raw::get_string(&mut text)` and the existing length check at line 125 (`text.len() > MAX_CLIPBOARD_TEXT_BYTES`), which checks the actual decoded UTF-8 string length.
- **Confidence**: high

### [P1] macOS input injector lacks Drop implementation, leaving keys and mouse buttons stuck on session disconnect
- **Location**: `clients/rust/maho-host/src/inject_macos.rs:30-44`
- **Evidence**:
```rust
pub struct InputInjector {
    host_width: f32,
    host_height: f32,
    rate_limit: Mutex<RateLimit>,
    // CGEventSource is !Send/!Sync: create, use, and drop on the session thread.
    #[cfg(target_os = "macos")]
    event_source: std::cell::RefCell<Option<core_graphics::event_source::CGEventSource>>,
    #[cfg(all(test, target_os = "macos"))]
    source_creations: std::cell::Cell<usize>,
    // Track held mouse buttons and keys for Reset event
    #[cfg(target_os = "macos")]
    active_mouse_buttons: std::cell::RefCell<HashSet<u8>>,
    #[cfg(target_os = "macos")]
    active_keys: std::cell::RefCell<HashSet<u16>>,
}
```
- **Impact**: `inject_macos.rs` maintains `active_mouse_buttons` and `active_keys` to synthesize releases on `Reset`, but implements no `Drop` trait for `InputInjector`. On Linux, closing the `/dev/uinput` file descriptor automatically removes virtual devices and clears held inputs; on Windows, `WindowsInputInjector::drop` releases held modifiers. On macOS, events are injected directly into the system-wide Quartz HID tap (`CGEventTapLocation::HID`). If a remote session terminates abnormally (network disconnect, client crash, browser tab close during drag or key press), `InputInjector` is dropped without releasing inputs. Held mouse buttons remain pressed (dragging windows or selecting text across macOS), and held keys continue repeating indefinitely until cleared by physical user input on the host keyboard/mouse.
- **Fix**: Implement `Drop for InputInjector` on macOS to iterate through `self.active_mouse_buttons` and `self.active_keys` and post `LeftMouseUp`, `RightMouseUp`, `OtherMouseUp`, and key-up events to `CGEventTapLocation::HID`, followed by clearing the sets.
- **Confidence**: high

### [P1] Linux multi-monitor negative desktop offsets clamp pointer coordinates to zero, trapping cursor on secondary displays
- **Location**: `clients/rust/maho-host/src/inject_linux.rs:62-65`
- **Evidence**:
```rust
    (
        (i64::from(geometry.x) + x.round() as i64).max(0) as u32,
        (i64::from(geometry.y) + y.round() as i64).max(0) as u32,
    )
```
- **Impact**: In Linux multi-monitor configurations (e.g. Hyprland, Sway, GNOME Wayland), displays positioned to the left of or above the primary display have negative logical coordinates (`geometry.x < 0` or `geometry.y < 0`). For example, a 1920x1080 display placed immediately left of the primary display has `geometry.x = -1920`. For any normalized coordinate `normalized_x in [0.0, 1.0]`, `geometry.x + x` is in `[-1920, -1]`. Calling `.max(0) as u32` forces all X coordinates on that display to `0`. As a result, all mouse moves, drags, and clicks on negative-offset displays are clamped to `x = 0`, completely breaking pointer input injection on secondary displays.
- **Fix**: Either configure the absolute uinput device min/max bounds to cover the signed global desktop range `[desktop_min_x..desktop_max_x]`, or map local display coordinates using an unsigned desktop coordinate space shifted by `desktop_min_x` (analogous to `normalize_absolute_pointer` in `windows_logic.rs:137-142`).
- **Confidence**: high

### [P2] macOS UnicodeChar event posts key-down without matching key-up, causing stuck virtual key 0 and typematic repeats
- **Location**: `clients/rust/maho-host/src/inject_macos.rs:288-295`
- **Evidence**:
```rust
            InputEventType::UnicodeChar => {
                // Convert UTF-16 code unit to string and set on keyboard event
                let unicode_str = String::from_utf16_lossy(&[event.key_code]);
                let cg_event = CGEvent::new_keyboard_event(source()?, 0, true)
                    .map_err(|_| InputError::EventCreation)?;
                cg_event.set_string(&unicode_str);
                return Ok(Some(cg_event));
            }
```
- **Impact**: When injecting a `UnicodeChar`, `inject_macos.rs` creates a `CGEvent` with `keyDown = true` for virtual key 0 (`kVK_ANSI_A`), sets the Unicode character string, and returns it. No corresponding key-up event is ever generated or posted. Furthermore, key 0 is not recorded in `active_keys`, so subsequent `Reset` events cannot release it. The macOS HID system treats virtual key 0 as held down, which can trigger unwanted typematic repeats or corrupt subsequent keystrokes until key 0 is physically pressed and released.
- **Fix**: Post the key-down event and immediately post a matching key-up event (`CGEvent::new_keyboard_event(source()?, 0, false)`), matching the behavior of Windows and Linux injectors.
- **Confidence**: high

### [P2] Linux UnicodeChar emits KeyDown and KeyUp in a single evdev batch, violating protocol and risking dropped keypresses
- **Location**: `clients/rust/maho-host/src/inject_linux.rs:557-567`
- **Evidence**:
```rust
                    if let Some((key, needs_shift)) = ascii_to_evdev(ch) {
                        let mut events = Vec::with_capacity(4);
                        if needs_shift {
                            events.push(InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1));
                        }
                        events.push(InputEvent::new(EventType::KEY.0, key.code(), 1));
                        events.push(InputEvent::new(EventType::KEY.0, key.code(), 0));
                        if needs_shift {
                            events.push(InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0));
                        }
                        self.keyboard.emit(&events)?;
```
- **Impact**: `self.keyboard.emit(&events)` batches all events and flushes them with a single terminating `SYN_REPORT`. In the Linux evdev subsystem, events within a single synchronization report represent simultaneous state changes. Emitting `value = 1` and `value = 0` for the same key code in the same report violates evdev semantics: input consumers (libinput, Xwayland, Wayland compositors) see only the final state (`0`) or treat the conflicting transition as a dropped event. Characters injected via `UnicodeChar` may be dropped intermittently.
- **Fix**: Split the key-down and key-up into two distinct `emit()` calls so each transition is terminated with its own `SYN_REPORT`:
```rust
if needs_shift { self.keyboard.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 1)])?; }
self.keyboard.emit(&[InputEvent::new(EventType::KEY.0, key.code(), 1)])?;
self.keyboard.emit(&[InputEvent::new(EventType::KEY.0, key.code(), 0)])?;
if needs_shift { self.keyboard.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTSHIFT.code(), 0)])?; }
```
- **Confidence**: high

### [P2] WASAPI loopback format parsing ignores bits-per-sample and sample rate, risking zero division panic and audio corruption
- **Location**: `clients/rust/maho-host/src/audio_windows.rs:149-173`
- **Evidence**:
```rust
    fn parse_mix_format(format: *mut WAVEFORMATEX) -> Result<SourceAudioFormat, String> {
        // SAFETY: the mix format pointer is a valid WAVEFORMATEX from WASAPI.
        let header = unsafe { &*format };
        let is_float = match header.wFormatTag {
            WAVE_FORMAT_IEEE_FLOAT => true,
            WAVE_FORMAT_EXTENSIBLE => {
                // Packed layout: never form references to its fields.
                // SAFETY: WASAPI guarantees an extensible tag carries a
                // WAVEFORMATEXTENSIBLE layout.
                let extensible = format.cast::<WAVEFORMATEXTENSIBLE>();
                let sub_format =
                    unsafe { core::ptr::addr_of!((*extensible).SubFormat).read_unaligned() };
                sub_format == IEEE_FLOAT_SUBFORMAT
            }
            WAVE_FORMAT_PCM => false,
            tag => return Err(format!("unsupported mix format tag: {tag:#x}")),
        };
        let channels = header.nChannels as usize;
        if channels == 0 {
            return Err("mix format has no channels".into());
        }
        Ok(SourceAudioFormat {
            channels,
            sample_rate: header.nSamplesPerSec,
            is_float,
        })
    }
```
- **Impact**: `parse_mix_format` validates `channels > 0`, but does not validate that `header.nSamplesPerSec > 0`. If a virtual or buggy audio driver reports `nSamplesPerSec == 0`, `convert_to_wire_audio` in `windows_logic.rs:56` executes integer division by zero (`output_frames = ... / u64::from(source.sample_rate)`), crashing the host process. Furthermore, when `is_float` is false, `convert_to_wire_audio` assumes 16-bit integer PCM, ignoring `wBitsPerSample`. If an endpoint outputs 24-bit or 32-bit integer PCM, the byte offsets become misaligned, producing severe acoustic distortion. Additionally, if `parse_mix_format` returns `Err`, `CoTaskMemFree` in `new()` is bypassed, leaking the `WAVEFORMATEX` memory block allocated by WASAPI.
- **Fix**: Validate `header.nSamplesPerSec > 0` and verify that `header.wBitsPerSample == 16` for PCM (or support 24/32-bit conversion). Ensure `format_ptr` is wrapped in a guard that calls `CoTaskMemFree` on error exit.
- **Confidence**: high

### [P2] Linux audio capture fails without parec fallback when pactl is missing or non-executable
- **Location**: `clients/rust/maho-host/src/audio_linux.rs:95-109` and `clients/rust/maho-host/src/audio_linux.rs:356-362`
- **Evidence**:
```rust
        if command_available("parec") {
            let source = match env::var("MAHO_AUDIO_MONITOR").ok() {
                Some(source) => Some(source),
                None => default_pulse_monitor_source(stop)?,
            };
            let mut command = Command::new("parec");
            command.args([
                "--raw",
                "--format=float32ne",
                "--rate=48000",
                "--channels=2",
            ]);
            if let Some(source) = source {
                command.arg(format!("--device={source}"));
            }
            return Self::spawn_cancellable(AudioBackend::PulseAudio, command, stop);
        }
```
and:
```rust
fn command_available(name: &str) -> bool {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .any(|candidate| candidate.is_file())
}
```
- **Impact**: When `pw-record` is unavailable and `MAHO_AUDIO_MONITOR` is unset, `open_cancellable` checks `command_available("parec")`. However, `default_pulse_monitor_source(stop)?` immediately spawns `pactl get-default-sink`. If `pactl` is not installed or not executable, `pactl` spawn fails with `io::ErrorKind::NotFound`, and the `?` operator aborts `open_cancellable` with `AudioError::Io`. `parec` works natively without `--device` by capturing from the default monitor source, but `audio_linux.rs` never attempts to spawn `parec` without `--device`. Furthermore, `command_available` only checks `candidate.is_file()` without verifying execute permissions (`mode & 0o111 != 0`), causing non-executable candidates in PATH to fail with `PermissionDenied`.
- **Fix**: Handle `default_pulse_monitor_source` failure by falling back to `None` (invoking `parec` without `--device`) instead of propagating the error with `?`. Update `command_available` to check executable permissions.
- **Confidence**: high

### [P2] macOS input injector ignores display origin offsets in multi-monitor setups, mapping secondary display input to primary display
- **Location**: `clients/rust/maho-host/src/inject_macos.rs:66-78`
- **Evidence**:
```rust
    pub fn map_coordinates(&self, event: &InputEvent) -> Result<(f32, f32), InputError> {
        if !event.x.is_finite() || !event.y.is_finite() {
            return Err(InputError::InvalidCoordinates);
        }
        // Protocol convention inverts Y (1.0 - y) on wire for legacy compatibility.
        // Invert it back so (0,0) is top-left in Quartz/CoreGraphics display coordinates.
        Ok(map_to_host_pixels(
            event.x.clamp(0.0, 1.0),
            (1.0 - event.y).clamp(0.0, 1.0),
            self.host_width,
            self.host_height,
        ))
    }
```
- **Impact**: `InputInjector` stores only `host_width` and `host_height`, omitting display origin offsets (`desktop_x`, `desktop_y`). In macOS, CoreGraphics global display coordinates place `(0, 0)` at the top-left of the primary display, while secondary displays have non-zero origins (e.g. `(1920, 0)` or `(-1920, 0)`). When capturing and streaming a secondary display, normalized wire coordinates are mapped directly to `(0..host_width, 0..host_height)` relative to `(0, 0)`. Consequently, all injected mouse clicks and movements land on the primary display rather than the secondary display being streamed.
- **Fix**: Accept display origin coordinates `(origin_x, origin_y)` in `InputInjector::new` and add them to the pixel coordinates in `map_coordinates`.
- **Confidence**: high

### [P2] macOS injector drops RelativeMove events as silent no-ops, breaking pointer lock in games and 3D applications
- **Location**: `clients/rust/maho-host/src/inject_macos.rs:297-301`
- **Evidence**:
```rust
            InputEventType::Reset
            | InputEventType::RelativeMove
            | InputEventType::GamepadAxis
            | InputEventType::GamepadButtonDown
            | InputEventType::GamepadButtonUp => return Ok(None),
```
- **Impact**: When a remote user activates pointer lock (e.g. in 3D web applications, first-person games, or virtual camera controls), the client sends `InputEventType::RelativeMove` with mouse delta coordinates in `scroll_dx` and `scroll_dy`. While Windows (`inject_windows.rs:90`) and Linux (`inject_linux.rs:446`) implement relative motion injection, `inject_macos.rs` explicitly treats `RelativeMove` as an unhandled no-op returning `Ok(None)`. Pointer-locked interaction on macOS hosts is completely non-functional.
- **Fix**: Implement `RelativeMove` by querying the current cursor position via `CGEvent::new(None)` and posting a mouse moved event with relative deltas via `CGEventSetIntegerValueField(..., kCGMouseEventDeltaX, dx)` and `kCGMouseEventDeltaY`, or warping cursor position.
- **Confidence**: high

## Non-findings checked
- The wire Y-inversion convention `(1.0 - y)` is consistently inverted back across macOS (`inject_macos.rs:73`), Linux (`inject_linux.rs:49`), and Windows (`windows_logic.rs:136`) for absolute pointer motion, clicks, and drags.
- Finite coordinate validation: all three host input injectors (`inject_macos.rs:68`, `inject_linux.rs:408`, `inject_windows.rs:74`) validate `is_finite()` on coordinates and deltas, rejecting `NaN` and infinity before dispatch.
- Linux audio child process zombie reaping on normal shutdown: `LinuxAudioCapture::shutdown` polls `child.try_wait()` with kill escalation and a 1-second deadline to ensure child processes (`pw-record`/`parec`/`pactl`) are reaped upon closure.
- Windows sensitive clipboard exclusion formats: `clipboard_windows.rs` registers and verifies `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory`, and `CanUploadToCloudClipboard`, suppressing synchronization of password manager entries.
- PipeWire/PulseAudio capture format matching: both `pw-record` and `parec` commands specify 48 kHz, stereo, native float32, matching the protocol's audio media contract.
- Windows SendInput return value verification: `send_inputs` asserts `sent == inputs.len() as u32` and extracts `io::Error::last_os_error()` on partial or failed injection.
- Linux uinput cleanup on drop: `VirtualDevice` struct wrappers drop their underlying file descriptors, removing virtual devices from the kernel input subsystem.
- Linux audio read frame alignment: `read_interleaved_f32_cancellable` truncates pending bytes to multiples of `BYTES_PER_SAMPLE_FRAME` (8 bytes), preventing channel-swap desynchronization on partial pipe reads.
