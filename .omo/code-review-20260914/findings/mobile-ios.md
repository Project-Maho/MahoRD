# Lane: mobile-ios

## Scope reviewed

- `clients/rust/maho-mobile/src/android.rs`: 176 lines
- `clients/rust/maho-mobile/src/bridge.rs`: 158 lines
- `clients/rust/maho-mobile/src/ios.rs`: 160 lines
- `clients/rust/maho-mobile/src/keyboard.rs`: 167 lines
- `clients/rust/maho-mobile/src/lib.rs`: 19 lines
- `clients/rust/maho-mobile/src/lifecycle.rs`: 164 lines
- `clients/rust/maho-mobile/src/power.rs`: 164 lines
- `clients/rust/maho-mobile/src/storage.rs`: 529 lines
- `clients/rust/maho-mobile/src/touch.rs`: 501 lines
- `clients/rust/ios-shell/src/commands.rs`: 126 lines
- `clients/rust/ios-shell/src/frame.rs`: 51 lines
- `clients/rust/ios-shell/src/lib.rs`: 35 lines
- `clients/rust/ios-shell/src/main.rs`: 3 lines
- `clients/rust/ios-shell/src/qa.rs`: 99 lines
- `clients/rust/ios-shell/src/state.rs`: 1543 lines
- `clients/rust/ios-shell/src/tests.rs`: 539 lines
- `clients/rust/ios-shell/tests/lifecycle.rs`: 658 lines
- `clients/rust/maho-app/src/pairing.rs`: 926 lines (imported: pairing storage backend & Keychain FFI)
- `clients/rust/maho-decode/src/vt/mod.rs`: 561 lines (imported: VideoToolbox FFI decoder implementation)
- `clients/rust/maho-render/src/audio.rs`: 498 lines (imported: iOS AVAudioSession activation FFI)
- `clients/rust/ios-shell/ui/lifecycle.js`: 94 lines (imported: webview lifecycle handlers)
- `clients/rust/ios-shell/ui/connection-state.js`: 742 lines (imported: client state manager)
- `clients/rust/ios-shell/ui/touch-coords.js`: 87 lines (imported: aspect fit & touch coordinates)

## Findings

### P1 Missing Legacy Keychain Service Migration After 60b02ca Rebrand Causes Credential Loss on iOS
- **Location**: `clients/rust/maho-app/src/pairing.rs:338` (and secondary site `clients/rust/maho-mobile/src/storage.rs:117`)
- **Evidence**:
```rust
    pub fn open_default() -> Result<Self, PairingStoreError> {
        #[cfg(target_os = "ios")]
        {
            Ok(Self::new_keychain("com.projectmaho.mahord.pairing"))
        }
        #[cfg(not(target_os = "ios"))]
        {
            Ok(Self::new(Self::default_path()?))
        }
    }
```
and in `clients/rust/maho-app/src/pairing.rs:698-700`:
```rust
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn save_keychain(service: &str, record: &PairingRecord) -> Result<(), PairingStoreError> {
    let key = format!("maho_pairing_{}", record.id);
```
- **Impact**: In commit 60b02ca, the Keychain service namespace was mechanically renamed from `com.eclipticrd.ios.pairing` to `com.projectmaho.mahord.pairing`, and account keys from `erd_pairing_*` to `maho_pairing_*`. While file-based storage in `PairingStore::load_all()` has an explicit migration branch importing `pairing-keys.json` into `client-pairings.json` (lines 352–368), Keychain storage has no fallback or migration query. When existing iOS users update the application, `load_all_keychain()` and `load_keychain()` query only the new service name, abandoning all previously established host pairing keys and endpoint metadata in the device Keychain. Reconnection without a PIN fails, forcing users to re-pair with an 8-digit PIN for all saved hosts.
- **Fix**: In `load_all_keychain` (or during iOS startup initialization in `PairingStore::open_default`), if querying `com.projectmaho.mahord.pairing` yields zero records, query the legacy service `com.eclipticrd.ios.pairing` (for accounts with prefix `erd_pairing_` or `maho_pairing_`). Decode any existing records, save them into `com.projectmaho.mahord.pairing`, and preserve the legacy Keychain items to ensure seamless upgrades without authentication loss.
- **Confidence**: High

### P1 Stale Teardown Generation Overwrites `TeardownCoordInner.done_generation`, Breaking Teardown Idempotency
- **Location**: `clients/rust/ios-shell/src/state.rs:274` (and secondary site `clients/rust/ios-shell/src/state.rs:248`)
- **Evidence**:
```rust
        // 2. If this generation was already reaped:
        if coord.done_generation == Some(generation) {
            let last_res = coord.last_result.clone().unwrap_or(Ok(()));
            if let Err(ref e) = last_res {
                return Err(e.clone());
            }
            if target_state == ConnectionState::Idle {
                let mut inner = self.inner.lock().map_err(|e| format!("State mutex poisoned: {e}"))?;
                if inner.generation == generation {
                    inner.state = ConnectionState::Idle;
                    inner.session = None;
                    inner.host = None;
                    inner.first_frame_presented.store(false, Ordering::Relaxed);
                }
            }
            return Ok(());
        }

        // 3. We become the teardown owner for this generation:
        coord.in_flight_generation = Some(generation);
        drop(coord);

        let reap_result = self.execute_teardown(generation, target_state, terminal_reason);

        // Record completion and wake up all waiting threads:
        let mut coord = lock.lock().map_err(|e| format!("Teardown lock poisoned: {e}"))?;
        coord.in_flight_generation = None;
        coord.done_generation = Some(generation);
        coord.last_result = Some(reap_result.clone());
        cvar.notify_all();
```
- **Impact**: `handle_terminal_shutdown` tracks reaped generations via `coord.done_generation: Option<u64>`. When a delayed worker from a previous generation (e.g. Gen 1) unblocks after Gen 2 has already shut down, `coord.done_generation` holds `Some(2)`. Because `coord.done_generation == Some(1)` is false, the stale Gen 1 thread acquires the teardown owner slot and calls `execute_teardown(1, ...)`. `execute_teardown` detects `inner.generation != generation` and returns `Ok(())`. However, line 274 unconditionally overwrites `coord.done_generation = Some(1)`. If Gen 2 subsequently receives a secondary termination event (e.g. concurrent `disconnect_blocking` after TCP remote close), line 248 checks `coord.done_generation == Some(2)`, which now evaluates to false. This causes `execute_teardown` to execute a second time on an already reaped generation, racing with any newly starting session in Gen 3.
- **Fix**: In `handle_terminal_shutdown`, verify that `generation == inner.generation` before taking the teardown owner slot, and only update `coord.done_generation` if `generation >= coord.done_generation.unwrap_or(0)`. If `generation < inner.generation`, return `Ok(())` immediately without modifying coordination state.
- **Confidence**: High

### P1 Unhandled Failure Spawning `supervisor_worker` Leaks TCP Runtime and Leaves Session in Stale `Ready` State
- **Location**: `clients/rust/ios-shell/src/state.rs:1073` (and lines 1011–1014)
- **Evidence**:
```rust
            inner.state = ConnectionState::Ready;
            inner.session = Some(session.clone());
            inner.tcp_runtime = Some(tcp_runtime);
            (inner.stop_flag.clone(), inner.worker_completions.clone())
        };

        // 1. Supervisor worker consuming RuntimeEvents:
        let state_supervisor = self.clone();
        let stop_supervisor = stop_flag.clone();
        let event_rx = {
            let inner = self.inner.lock().unwrap();
            inner.tcp_runtime.as_ref().unwrap().events().clone()
        };
        let supervisor_completions = worker_completions.clone();
        let supervisor_worker = thread::Builder::new()
            .name("maho-ios-supervisor".to_string())
            .spawn(move || {
```
and line 1073:
```rust
            .map_err(|e| IpcError::connection_failed(IpcErrorStage::Runtime, format!("Failed to spawn supervisor worker: {e}")))?;
```
- **Impact**: In `connect_blocking`, line 1011 commits `inner.state = ConnectionState::Ready`, sets `inner.session`, and sets `inner.tcp_runtime`. If spawning `supervisor_worker` fails on line 1073 (e.g. hitting OS thread/task exhaustion), the error is propagated via `?` without rollback. Unlike `audio_worker` (lines 1177–1180) and `media_worker` (lines 1357–1360) which call `handle_terminal_shutdown` on spawn failure, `supervisor_worker` leaves the session marked `Ready` with an active background `tcp_runtime` and live TCP stream that has no supervisor to monitor disconnects or errors. Subsequent calls to `stats()` report `"ready"` to the UI, and input handlers attempt to dispatch over an unmonitored connection.
- **Fix**: Wrap `supervisor_worker` spawn error with rollback logic matching the audio and media workers:
```rust
            .map_err(|e| {
                let _ = self.handle_terminal_shutdown(
                    current_generation,
                    ConnectionState::Error,
                    Some(format!("Failed to spawn supervisor worker: {e}")),
                );
                IpcError::connection_failed(IpcErrorStage::Runtime, format!("Failed to spawn supervisor worker: {e}"))
            })?;
```
- **Confidence**: High

### P2 Non-Finite Coordinates in `TouchPhase::Ended` Prevent Mouse Release and Lock All Future Touches
- **Location**: `clients/rust/maho-mobile/src/touch.rs:261` (and lines 314–334)
- **Evidence**:
```rust
        if !touch.x.is_finite() || !touch.y.is_finite() {
            return Err(TouchError::NonFiniteCoordinates(touch.x, touch.y));
        }

        match touch.phase {
```
and in `clients/rust/maho-mobile/src/touch.rs:314–334`:
```rust
            TouchPhase::Ended => {
                if self.mode == TouchMode::DirectTouch {
                    if let Some(primary) = self.primary_touch {
                        if primary.id == touch.id {
                            self.primary_touch = None;
                            let (release_x, release_y) = self
                                .viewport
                                .transform_to_host(touch.x, touch.y)
                                .unwrap_or((
                                    primary.last_emitted_norm_x,
                                    primary.last_emitted_norm_y,
                                ));
                            return Ok(Some(make_mouse_event(
                                InputEventType::LeftMouseUp,
                                release_x,
                                release_y,
                            )));
                        }
                    }
                }
```
- **Impact**: In `TouchGestureHandler::process_touch`, `TouchPhase::Cancelled` is safely handled at lines 239–259 before coordinate validation. In contrast, `TouchPhase::Ended` occurs after line 261. If a touch release event is dispatched with non-finite coordinates (`NaN` or `Infinity` from client-side calculations when viewport or rect dimensions temporarily collapse to 0 during orientation change), line 261 returns an error. As a result, line 318 (`self.primary_touch = None`) is never executed, and `LeftMouseUp` is never emitted to the host. Because `self.primary_touch` remains set, the remote host mouse button is permanently stuck down, and line 267 (`if self.primary_touch.is_none()`) permanently drops all subsequent touches until the application restarts or switches modes.
- **Fix**: Check coordinate finiteness only within `TouchPhase::Began` and `TouchPhase::Moved`. Allow `TouchPhase::Ended` to proceed when coordinates are non-finite by taking `self.primary_touch` and falling back to `(primary.last_emitted_norm_x, primary.last_emitted_norm_y)` (which line 324 was already designed to do).
- **Confidence**: High

### P2 `audio_events_sender` Is Never Cleared on Teardown, Retaining Stale SyncSender Across Generations
- **Location**: `clients/rust/ios-shell/src/state.rs:346` (and lines 1082–1085)
- **Evidence**:
```rust
            let _ = inner.audio_queue.clear();
            if let Ok(mut frame_guard) = inner.latest_frame.lock() {
                *frame_guard = None;
            }

            let sess = inner.session.take();
            let tcp_rt = inner.tcp_runtime.take();
            let handles = std::mem::take(&mut inner.worker_handles);
```
and in `clients/rust/ios-shell/src/state.rs:1082–1085`:
```rust
            inner.worker_handles.push(supervisor_worker);
            if let Ok(mut sender_guard) = inner.audio_events_sender.lock() {
                *sender_guard = Some(audio_events_tx.clone());
            };
```
- **Impact**: When a session is torn down in `execute_teardown`, `audio_queue`, `latest_frame`, `session`, `tcp_runtime`, and `worker_handles` are cleaned up, but `inner.audio_events_sender` is not cleared. It continues holding `Some(audio_events_tx.clone())` from the terminated session until a new connection succeeds. If `inject_audio_event_for_test` is called while the app is in `Idle` or `Error` state, it sends into the orphan channel rather than returning `"No active audio event sender"`. Additionally, retaining this sender prevents `audio_events_rx` in any exiting audio thread from observing channel disconnection.
- **Fix**: In `execute_teardown` (around line 348), clear `audio_events_sender`:
```rust
            if let Ok(mut sender_guard) = inner.audio_events_sender.lock() {
                *sender_guard = None;
            }
```
- **Confidence**: High

### P3 `maho-mobile::storage` Duplicates `maho-app::pairing` with Inefficient N+1 Lookups and Missing Endpoint Cache
- **Location**: `clients/rust/maho-mobile/src/storage.rs:18` (and lines 94–109)
- **Evidence**:
```rust
pub trait SecureStorageBackend: Send + Sync {
    fn store_secret(&self, key: &str, secret: &[u8]) -> Result<(), SecureStorageError>;
    fn retrieve_secret(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError>;
    fn delete_secret(&self, key: &str) -> Result<(), SecureStorageError>;
    fn list_keys(&self) -> Result<Vec<String>, SecureStorageError>;
}
```
and lines 94–106:
```rust
    pub fn load_all(&self) -> Result<Vec<PairingRecord>, SecureStorageError> {
        let mut records = Vec::new();
        for key in self.backend.list_keys()? {
            if key.starts_with("maho_pairing_") {
                if let Some(bytes) = self.backend.retrieve_secret(&key)? {
                    if let Ok(record) = serde_json::from_slice::<PairingRecord>(&bytes) {
                        if record.key_array().is_ok() {
                            records.push(record);
                        }
                    }
                }
            }
        }
        Ok(records)
    }
```
- **Impact**: `maho-mobile` contains a standalone 529-line secure storage subsystem (`MobilePairingStore`, `IosKeychainStorage`, `MockSecureStorage`, and duplicate `security_ffi` declarations) that is completely unused by `ios-shell` (which relies exclusively on `maho_app::PairingStore`). `MobilePairingStore` diverges significantly from production requirements: its `load_all` executes N+1 IPC calls against the Keychain rather than `maho_app`'s single batch query, it lacks `remember_endpoint` / alias management, and its `default_keychain()` on non-Apple targets silently falls back to an ephemeral in-memory HashMap (`MockSecureStorage`).
- **Fix**: Remove the duplicated storage module in `maho-mobile` or refactor `MobilePairingStore` to be a thin adapter over `maho_app::PairingStore`.
- **Confidence**: High

### P3 Unused Media Stubs and Lifecycle Types in `maho-mobile` Diverge from Production iOS Shell
- **Location**: `clients/rust/maho-mobile/src/ios.rs:59` (and `clients/rust/maho-mobile/src/lifecycle.rs:26`)
- **Evidence**:
```rust
    pub fn render_frame(&mut self, payload: &[u8]) -> Result<bool, IosMediaError> {
        if !self.config.metal_layer_attached {
            return Err(IosMediaError::MetalLayerMissing);
        }
        if payload.is_empty() {
            return Ok(false);
        }
        Err(IosMediaError::BackendUnavailable)
    }
```
and `clients/rust/maho-mobile/src/lifecycle.rs:26–36`:
```rust
pub struct MobileLifecycleManager {
    orientation: DeviceOrientation,
    state: AppLifecycleState,
    network_type: NetworkInterfaceType,
    last_state_change: Instant,
    needs_keyframe_on_resume: bool,
    reconnect_attempts: u32,
    max_reconnect_attempts: u32,
}
```
- **Impact**: `maho-mobile` exports `IosVideoToolboxDecoder`, `IosAudioEnginePlayer`, and `MobileLifecycleManager`. None of these are used by the actual iOS application: `ios-shell` performs hardware video decoding using `maho_decode::HevcDecoder` (VTDecompressionSession) and audio playback via `maho_render::CpalAudioOutput`, while app lifecycle is handled via webview events in `ui/lifecycle.js`. The unused types in `maho-mobile` provide no operational value and create confusion about where mobile platform features are implemented.
- **Fix**: Remove the inactive stubs from `maho-mobile`, or integrate `MobileLifecycleManager` into `ios-shell` to handle native backgrounding notifications (`UIApplicationDidEnterBackgroundNotification`).
- **Confidence**: High

## Non-findings checked

- `maho_mobile_create` and `maho_mobile_destroy` pointer safety: validated null-pointer checks, UTF-8 conversion, slot initialization to null, and `Box::from_raw` lifecycle guarantees.
- VideoToolbox decoder teardown in `maho-decode/src/vt/mod.rs`: verified `VTDecompressionSessionInvalidate` and async frame drains are called before releasing format descriptions or reclaiming `SharedState` context.
- Coordinate bounding in `clients/rust/ios-shell/ui/touch-coords.js`: verified aspect fit calculations clamp display dimensions to positive values and avoid division by zero on non-finite or zero-sized containers.
- NV12 frame repacking buffer safety in `clients/rust/ios-shell/src/frame.rs`: verified destination slices and row copy offsets do not panic on either matching or strided planes.
- Mutex ordering during teardown in `clients/rust/ios-shell/src/state.rs`: confirmed `inner` mutex is dropped prior to joining worker threads or stopping the TCP runtime, avoiding deadlock.
- Generation-aware worker completions in `ios-shell/src/state.rs`: verified `WorkerCompletion` tags every termination event with its spawning generation number so stale worker exits cannot terminate new sessions.
- Asynchronous cancelation before connect registration: verified `cancel_active_connect` sets atomic cancel flags and interrupts sockets immediately without waiting on `lifecycle_lock`.
- Single primary touch ownership in `TouchGestureHandler`: verified secondary finger events during a held drag cannot prematurely release or steal the active touch point.
- Integer overflow prevention in `maho-mobile/src/power.rs`: verified quotient/remainder arithmetic in `Fair` thermal scaling prevents `u32` overflow even at `u32::MAX`.
- QA provisioning gate in `clients/rust/ios-shell/src/qa.rs`: confirmed provisioning is restricted to `#[cfg(debug_assertions)]`, deletes the provisioning file immediately, and validates Keychain saving before auto-connecting.
