# Lane: frontend-core

## Scope reviewed
- `clients/rust/tauri-shell/src/lib/renderer.ts` (378 lines)
- `clients/rust/tauri-shell/src/lib/renderer.test.ts` (311 lines)
- `clients/rust/tauri-shell/src/lib/connection.ts` (318 lines)
- `clients/rust/tauri-shell/src/lib/connection.test.ts` (415 lines)
- `clients/rust/tauri-shell/src/lib/ipc.ts` (363 lines)
- `clients/rust/tauri-shell/src/lib/ipc.test.ts` (404 lines)
- `clients/rust/tauri-shell/src/lib/overlay.ts` (298 lines)
- `clients/rust/tauri-shell/src/lib/overlay.test.ts` (235 lines)
- `clients/rust/tauri-shell/src/lib/library.ts` (215 lines)
- `clients/rust/tauri-shell/src/lib/library.test.ts` (284 lines)
- `clients/rust/tauri-shell/src/lib/utils.ts` (6 lines)

Direct consumers / producers inspected to evaluate interface boundaries and runtime behavior:
- `clients/rust/tauri-shell/src-tauri/src/lib.rs` (Media decode, NV12 payload packing, lifecycle locking)
- `clients/rust/tauri-shell/src/features/session/SessionCanvas.tsx` (Renderer lifecycle, frame loop, cursor updates)
- `clients/rust/tauri-shell/src/features/session/SessionView.tsx` (Session view state, polling, input teardown)
- `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts` (Held input tracker integration, releaseAll)
- `clients/rust/tauri-shell/src/app/App.tsx` (Connection instance lifecycle, view routing)
- `clients/rust/tauri-shell/src/features/library/ComputersPage.tsx` (Connection initiation, error and busy handling)

## Findings

### [P0] Permanent UI deadlock when teardown or input release fails in connection cleanup
- **Location**: `clients/rust/tauri-shell/src/lib/connection.ts:208-216` (secondary: `clients/rust/tauri-shell/src/lib/connection.ts:221`)
- **Evidence**:
```ts
      if (errors.length) {
        state.cleanupError = errors.join('\n');
        state.phase = 'error';
      } else {
        state.busy = false;
        state.host = null;
        state.phase = state.error ? 'error' : 'idle';
      }
      pendingCleanup = null;
      emit();
```
```ts
  async function connect(request: ValidateConnectionOptions): Promise<boolean> {
    if (!nativeAvailable || state.busy) return false;
```
- **Impact**: When cleanup encounters any error during disconnect (such as `releaseInputs()` throwing or `invoke('disconnect')` failing), `state.busy` remains `true` indefinitely. Because `state.phase` transitions to `'error'`, `App.tsx` unmounts `SessionView` (which contains the Disconnect button) and navigates back to `ComputersPage`. On `ComputersPage`, every computer card and connect button is rendered with `busy={connSnapshot.busy || !native}`, disabling all user interaction. Furthermore, `connection.connect(...)` checks `if (!nativeAvailable || state.busy) return false;` and unconditionally rejects new connection attempts. There is no UI trigger, button, or retry mechanism anywhere in `ComputersPage` to invoke `retryCleanup()`. The entire application is rendered permanently deadlocked in a busy state, forcing the user to kill and restart the process.
- **Fix**: Either clear `state.busy = false` when transitioning to `'error'` so subsequent connection attempts can be initiated, or expose an error-clearing / retry action on `ComputersPage` when `connSnapshot.cleanupError` is non-null.
- **Confidence**: high

### [P1] Broken WebGL context-loss restoration freezes rendering permanently
- **Location**: `clients/rust/tauri-shell/src/lib/renderer.ts:281-288` (secondary: `clients/rust/tauri-shell/src/features/session/SessionCanvas.tsx:84-95`)
- **Evidence**:
```ts
  const onContextLost = (e: Event) => {
    e.preventDefault();
  };
  canvas.addEventListener("webglcontextlost", onContextLost, false);

  function renderNv12(width: number, height: number, yData: Uint8Array, uvData: Uint8Array): void {
    if (disposed || !gl || gl.isContextLost() || width <= 0 || height <= 0) return;
```
- **Impact**: Calling `e.preventDefault()` on `webglcontextlost` informs the browser that the client application intends to recover from context loss and expects a subsequent `webglcontextrestored` event. However, `renderer.ts` does not register a `webglcontextrestored` listener, nor does `RendererHandle` provide an event or callback to notify its consumer (`SessionCanvas.tsx`). When the GPU process restores the context, all existing WebGL programs, shaders, buffers, and textures (`yTexture`, `uvTexture`, `posBuf`, `texBuf`, `program`) remain invalid / destroyed handles. As soon as `gl.isContextLost()` returns `false`, `renderNv12` continues uploading frames, hitting `INVALID_OPERATION` on deleted textures and dead shader programs. Because `SessionCanvas.tsx` instantiates the renderer only once on mount (`useEffect(..., [])`), the video stream never recovers and stays permanently black or frozen until the session is terminated and reopened.
- **Fix**: Listen for `webglcontextrestored` on `canvas` to recreate shader programs, buffers, and textures (resetting `lastRenderWidth = 0; lastRenderHeight = 0`), or add an `onContextLost` callback to `createRenderer` options so `SessionCanvas` can dispose and recreate the renderer instance.
- **Confidence**: high

### [P1] NV12 buffer parsing framing mismatch drops odd-height frames and corrupts cursor metadata
- **Location**: `clients/rust/tauri-shell/src/lib/renderer.ts:98-107`, `clients/rust/tauri-shell/src/lib/renderer.ts:115-125` (secondary: `clients/rust/tauri-shell/src-tauri/src/lib.rs:1420-1424`, `clients/rust/tauri-shell/src-tauri/src/lib.rs:1443-1447`)
- **Evidence**:
```ts
    const yLen = width * height;
    const uvWidth = Math.floor(width / 2);
    const uvHeight = Math.ceil(height / 2);
    const uvLen = uvWidth * uvHeight * 2;
    const planesEnd = 16 + yLen + uvLen;

    if (!Number.isSafeInteger(planesEnd) || buf.byteLength < planesEnd) {
      return null;
    }
```
```ts
    let cursor: { x: number; y: number; visible: boolean } | null = null;
    if (buf.byteLength >= planesEnd + 9) {
      const cursorOffset = planesEnd;
      const cursorX = view.getFloat32(cursorOffset, true);
      const cursorY = view.getFloat32(cursorOffset + 4, true);
      const cursorType = view.getUint8(cursorOffset + 8);
      cursor = {
        x: cursorX,
        y: cursorY,
        visible: cursorType > 0,
      };
    }
```
From `src-tauri/src/lib.rs:1420-1424`:
```rust
                            let width = nv12.width as usize;
                            let height = nv12.height as usize;
                            let y_len = width * height;
                            let uv_len = width * (height / 2);
                            let total_bytes = 16 + y_len + uv_len + 9;
```
- **Impact**: The backend in `src-tauri/src/lib.rs` packs UV bytes using integer truncation `height / 2`, producing `uv_len = width * (height / 2)` and packing the 9-byte cursor metadata immediately following that offset. In `renderer.ts`, `parseFrame` calculates `uvHeight = Math.ceil(height / 2)`.
  1. For odd heights (e.g. 1081): Rust packs `width * 540` bytes, but TypeScript expects `width * 541` bytes. `buf.byteLength < planesEnd` evaluates to `true`, causing `parseFrame` to return `null`. Every single frame with an odd height received from the native backend is dropped and never displayed.
  2. For odd widths with even heights (e.g. 1921x1080): Rust packs `uv_len = 1921 * 540 = 1037340` bytes and writes the cursor at offset `16 + 2074680 + 1037340 = 3112036`. But TypeScript computes `uvLen = Math.floor(1921 / 2) * 540 * 2 = 960 * 540 * 2 = 1036800` bytes and computes `cursorOffset = planesEnd = 3111496`. The frontend reads `cursorX`, `cursorY`, and `cursorType` from byte offset 3111496—540 bytes earlier inside the active UV plane data. Raw chroma pixel bytes are interpreted as IEEE 754 floats, causing the remote cursor to jump to erratic coordinates or flicker with corrupted visibility.
- **Fix**: Align the buffer framing calculation between `src-tauri` and `renderer.ts`. In `parseFrame`, calculate `uvLen` matching the backend contract: `width * Math.floor(height / 2)` (or update `src-tauri` to use `height.div_ceil(2)` and `width.div_ceil(2) * 2` for true odd dimension support), and compute `cursorOffset` at `16 + yLen + uvLen`.
- **Confidence**: high

### [P1] WebGL2 UV texture upload lacks row stride handling on odd widths
- **Location**: `clients/rust/tauri-shell/src/lib/renderer.ts:289-290`, `clients/rust/tauri-shell/src/lib/renderer.ts:327-336`
- **Evidence**:
```ts
    const uvWidth = Math.floor(width / 2);
    const uvHeight = Math.ceil(height / 2);
```
```ts
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, uvTexture);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    if (isWebGL2) {
      const gl2 = gl as WebGL2RenderingContext;
      gl2.texSubImage2D(gl2.TEXTURE_2D, 0, 0, 0, uvWidth, uvHeight, gl2.RG, gl2.UNSIGNED_BYTE, uvData);
    } else {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, uvWidth, uvHeight, gl.LUMINANCE_ALPHA, gl.UNSIGNED_BYTE, uvData);
    }
```
- **Impact**: When the stream width is odd (e.g., 1921 pixels), each row in `uvData` has a stride of 1921 bytes on the wire. However, WebGL's default unpacking assumes contiguous rows of length `uvWidth * 2` bytes (e.g. `960 * 2 = 1920` bytes). Because `UNPACK_ROW_LENGTH` is never configured on WebGL2 (`gl2.pixelStorei(gl2.UNPACK_ROW_LENGTH, ...)`), WebGL advances 1920 bytes per row instead of 1921 bytes. Each successive UV row drifts by 1 byte relative to the previous row, inverting U and V chroma channels on alternating scanlines (creating chromatic rainbow/banding artifacts) and horizontally shearing color across the video surface.
- **Fix**: In WebGL2, set `gl2.pixelStorei(gl2.UNPACK_ROW_LENGTH, width / 2)` or pad/stride UV rows properly prior to `texSubImage2D`, and reset `UNPACK_ROW_LENGTH` to 0 after upload.
- **Confidence**: high

### [P2] Round-trip shader tests exercise disconnected local TypeScript helper instead of actual GLSL shaders
- **Location**: `clients/rust/tauri-shell/src/lib/renderer.test.ts:172-183`, `clients/rust/tauri-shell/src/lib/renderer.test.ts:234-252`
- **Evidence**:
```ts
function shaderNv12ToRgb(y: number, u: number, v: number): RgbPixel {
  // Texture sampling normalizes unsigned bytes [0, 255] to [0.0, 1.0]
  const texY = y / 255.0;
  const texU = u / 255.0;
  const texV = v / 255.0;

  // New full-range BT.601 shader math (no 16/255 offset, no 255/219 scaling, no 255/224 scaling)
  const shaderY = texY;
  const shaderU = texU - 0.5;
  const shaderV = texV - 0.5;

  const rNorm = clamp(shaderY + 1.402 * shaderV, 0.0, 1.0);
  const gNorm = clamp(shaderY - 0.344136 * shaderU - 0.714136 * shaderV, 0.0, 1.0);
  const bNorm = clamp(shaderY + 1.772 * shaderU, 0.0, 1.0);

  return {
    r: rNorm * 255.0,
    g: gNorm * 255.0,
    b: bNorm * 255.0,
  };
}
```
```ts
  for (const { name, r, g, b } of testCases) {
    it(`round-trips ${name} (r=${r}, g=${g}, b=${b}) within 3/255 tolerance`, () => {
      const { y, u, v } = hostBgraToNv12(r, g, b);
      const out = shaderNv12ToRgb(y, u, v);

      const errR = Math.abs(out.r - r);
      const errG = Math.abs(out.g - g);
      const errB = Math.abs(out.b - b);

      expect(errR).toBeLessThanOrEqual(TOLERANCE_255);
      expect(errG).toBeLessThanOrEqual(TOLERANCE_255);
      expect(errB).toBeLessThanOrEqual(TOLERANCE_255);

      expect(errR / 255).toBeLessThanOrEqual(TOLERANCE_NORM);
      expect(errG / 255).toBeLessThanOrEqual(TOLERANCE_NORM);
      expect(errB / 255).toBeLessThanOrEqual(TOLERANCE_NORM);
    });
  }
```
- **Impact**: The suite's round-trip verification tests (`BT.601 full-range NV12 round-trip`) assert math exclusively against `shaderNv12ToRgb()`, which is a local TypeScript mock function defined inside the test file itself. Neither `FS_SOURCE_WEBGL2` nor `FS_SOURCE_WEBGL1` from `renderer.ts` is imported, parsed, or executed by these tests. If a regression corrupts the shader source (e.g. Swapping U and V components, mutating matrix coefficients, or syntax errors in GLSL), all 6 round-trip tests continue to pass with 0 failures. The only test reading `renderer.ts` is `shader artifact range validation`, which merely performs naive substring matching (`.includes()`) for specific literal strings like `"255.0 / 219.0"`.
- **Fix**: Export the GLSL shader source strings from `renderer.ts` (or an intermediate shader math AST / evaluator) and validate the actual matrix coefficients extracted directly from the shader sources.
- **Confidence**: high

### [P2] Unit test for NV12 half-sampling duplicates parser flaw and cannot fail for backend mismatch
- **Location**: `clients/rust/tauri-shell/src/lib/renderer.test.ts:8-13`, `clients/rust/tauri-shell/src/lib/renderer.test.ts:138-152`
- **Evidence**:
```ts
  const yLen = width * height;
  const uvWidth = Math.floor(width / 2);
  const uvHeight = Math.ceil(height / 2);
  const uvLen = uvWidth * uvHeight * 2;
  const tailLen = tail ? (Array.isArray(tail) ? tail.length : 9) : 0;
  const totalLen = 16 + yLen + uvLen + tailLen;
```
```ts
  it("calculates UV dimensions matching NV12 half-sampling math", () => {
    const width = 65;
    const height = 47;
    const yLen = 65 * 47;
    const uvWidth = Math.floor(65 / 2); // 32
    const uvHeight = Math.ceil(47 / 2); // 24
    const uvLen = uvWidth * uvHeight * 2; // 1536
    const buf = createFrameBuffer(width, height);

    const parsed = parseFrame(buf);
    expect(parsed).not.toBeNull();
    expect(parsed?.width).toBe(65);
    expect(parsed?.height).toBe(47);
    expect(parsed?.y.byteLength).toBe(yLen);
    expect(parsed?.uv.byteLength).toBe(uvLen);
  });
```
- **Impact**: The test fixture helper `createFrameBuffer` in `renderer.test.ts` duplicates the same non-standard `uvHeight = Math.ceil(height / 2)` calculation present in `renderer.ts`. Because the test creates an artificial buffer structured to match the parser's internal assumption, the test passes green even though an actual frame generated with odd height by `src-tauri` (`uv_len = width * (height / 2) = 65 * 23 = 1495` bytes) fails validation and returns `null`. The test cannot fail for the wire format incompatibility it purports to validate.
- **Fix**: Test `parseFrame` with byte buffers generated according to the authoritative wire specification in `src-tauri/src/lib.rs`.
- **Confidence**: high

### [P2] Keyboard forwarding leaks remote inputs when focus sits on document.body
- **Location**: `clients/rust/tauri-shell/src/lib/overlay.ts:100-108`
- **Evidence**:
```ts
export function isRemoteInputTarget(target: unknown): boolean {
  if (!target || typeof (target as InputTargetLike).tagName !== 'string') {
    return false;
  }
  const el = target as InputTargetLike;
  if (el.tagName === 'BODY') {
    return true;
  }
  return typeof el.id === 'string' && REMOTE_TARGET_IDS.has(el.id as string);
}
```
- **Impact**: `isRemoteInputTarget` unconditionally treats `document.body` as a remote input target. Whenever focus shifts to `body`—which routinely occurs when a user clicks non-input elements in the overlay settings panel, dismisses a dropdown, or clicks outside a modal—`shouldForwardKeyboardEvent` returns `true`. Subsequent keystrokes (such as pressing space to scroll, Escape, or typing hotkeys) are forwarded as synthetic remote keystrokes to the host system instead of staying local to the shell UI, violating the overlay isolation guarantee.
- **Fix**: Only treat the canvas or container as a valid remote target (`REMOTE_TARGET_IDS.has(el.id)`), and remove unconditional forwarding on `BODY`, or gate `BODY` forwarding on the overlay panel not being expanded.
- **Confidence**: high

### [P2] Redundant frame parsing and typed array allocation on every frame in the hot path
- **Location**: `clients/rust/tauri-shell/src/lib/renderer.ts:367-372` (secondary: `clients/rust/tauri-shell/src/features/session/SessionCanvas.tsx:135-146`)
- **Evidence**:
```ts
    render(buf: ArrayBuffer): void {
      if (disposed) return;
      const frame = parseFrame(buf);
      if (!frame) return;
      renderNv12(frame.width, frame.height, frame.y, frame.uv);
    },
```
From `SessionCanvas.tsx:135-146`:
```tsx
        if (buf && buf.byteLength >= 16) {
          if (rendererRef.current) {
            rendererRef.current.render(buf);
          } else {
            onErrorRef.current?.(
              "Video rendering is unavailable (WebGL context could not be created)"
            );
          }

          const frame = parseFrame(buf);
          if (frame?.cursor) {
```
- **Impact**: Every frame polled at 60 FPS in `SessionCanvas.tsx` is parsed by `parseFrame` twice: first inside `renderer.render(buf)`, and immediately afterward in `SessionCanvas.tsx` to read `frame.cursor`. Each call to `parseFrame` creates a `DataView`, instantiates two `Uint8Array` buffer views (`y` and `uv`), and allocates an object wrapper. At 60 FPS (or 120 FPS on high-refresh displays), this duplicates parsing and object allocation 120–240 times per second on the rendering hot path, generating unnecessary garbage collection pauses during video streaming.
- **Fix**: Allow `RendererHandle.render` to accept an already-parsed `ParsedFrame` (or have `render` return the parsed frame / cursor), parsing the buffer exactly once per frame.
- **Confidence**: high

### [P2] createFavoritesStorage crashes on null/undefined storage parameter despite nullable type signature
- **Location**: `clients/rust/tauri-shell/src/lib/library.ts:95-105`
- **Evidence**:
```ts
export function createFavoritesStorage(storage?: StorageLike | null): FavoritesStorage {
  return {
    load(): string[] {
      const value = storage!.getItem(FAVORITES_KEY);
      return value === null ? [] : normalizedIps(JSON.parse(value));
    },
    save(ips: string[]): void {
      storage!.setItem(FAVORITES_KEY, JSON.stringify(normalizedIps(ips)));
    },
  };
}
```
- **Impact**: The function parameter explicitly permits `storage?: StorageLike | null`, indicating optional or nullable persistence storage. However, both `load()` and `save()` unconditionally invoke methods via non-null assertions `storage!.getItem` and `storage!.setItem`. If `createFavoritesStorage(null)` or `createFavoritesStorage()` is used directly, it crashes with an uncaught `TypeError: Cannot read properties of null/undefined (reading 'getItem')`.
- **Fix**: Provide an in-memory fallback storage adapter or guard against null/undefined `storage` inside `load()` and `save()`.
- **Confidence**: high

### [P3] validateConnection allows empty PIN and empty pairingId, delegating failure to backend
- **Location**: `clients/rust/tauri-shell/src/lib/connection.ts:86-88`, `clients/rust/tauri-shell/src/lib/connection.ts:118-123`
- **Evidence**:
```ts
  if (!host) errors.host = 'required';
  if (pin && !/^[0-9]{8}$/.test(pin)) errors.pin = 'invalid-pin';
```
```ts
  const finalTcp = checkPort(tcpPort, 'tcpPort', 19730);
  const finalUdp = checkPort(udpPort, 'udpPort', 19731);
  const args: ValidatedConnectionArgs = { host, tcpPort: finalTcp as number, udpPort: finalUdp as number, pin: pin || null };
  if (pairingId) args.pairingId = pairingId;
  return Object.keys(errors).length
    ? { ok: false, errors }
    : { ok: true, args };
```
- **Impact**: In `src-tauri/src/lib.rs:1234`, the backend strictly requires either a non-empty `pin` or a `pairing_id`: `if trimmed_pin.is_none() && trimmed_id.is_none() { return Err(IpcError::pairing_required("PIN required for initial authorization")); }`. However, `validateConnection` in `connection.ts` returns `{ ok: true }` when both `pin` and `pairingId` are omitted. The frontend client initiates an unnecessary network connect call that is guaranteed to fail in the native layer.
- **Fix**: In `validateConnection`, check `if (!pin && !pairingId) errors.pin = 'required';` when no existing pairing ID is supplied.
- **Confidence**: high

### [P3] invokeCommand omits window.__TAURI__.invoke fallback
- **Location**: `clients/rust/tauri-shell/src/lib/ipc.ts:275-288`
- **Evidence**:
```ts
  const tauri =
    typeof window !== "undefined" ? (window as any).__TAURI__ : undefined;
  const candidate = tauri?.core?.invoke ?? tauri?.tauri;
  const invokeFn =
    typeof candidate === "function"
      ? candidate
      : typeof candidate?.invoke === "function"
        ? candidate.invoke.bind(candidate)
        : undefined;
```
- **Impact**: In standard Tauri runtime environments where `window.__TAURI__.invoke` is exported directly on the `__TAURI__` namespace (rather than nested under `core`), `candidate` resolves to `undefined`. `invokeCommand` then throws an error saying Tauri IPC is not available, even when `window.__TAURI__.invoke` is present.
- **Fix**: Include `tauri?.invoke` in the resolution chain: `const candidate = tauri?.core?.invoke ?? tauri?.invoke ?? tauri?.tauri;`.
- **Confidence**: high

### [P3] buttonUpType returns null for numeric mouse buttons
- **Location**: `clients/rust/tauri-shell/src/lib/overlay.ts:88-93`
- **Evidence**:
```ts
export function buttonUpType(button: unknown): MouseUpEventType | null {
  if (button === 'left') return 'LeftMouseUp';
  if (button === 'middle') return 'MiddleMouseUp';
  if (button === 'right') return 'RightMouseUp';
  return null;
}
```
- **Impact**: While `normalizeMouseButton` accepts numeric button codes (0 for left, 1 for middle, 2 for right), the exported `buttonUpType(button: unknown)` only handles string identifiers. Passing a standard `MouseEvent.button` numeric code directly to `buttonUpType(0)` returns `null`.
- **Fix**: Normalize the button before mapping: `const norm = normalizeMouseButton(button); if (norm === 'left') ...`.
- **Confidence**: high

### [P3] selectHosts strictly compares boolean and rejects truthy numeric online status
- **Location**: `clients/rust/tauri-shell/src/lib/library.ts:80` (secondary: `clients/rust/tauri-shell/src/lib/library.ts:6`)
- **Evidence**:
```ts
      (!availableOnly || host.online === true) &&
```
From `library.ts:6`:
```ts
export interface Host {
  id: string;
  name?: string | null;
  ip: string;
  os?: string | null;
  online?: boolean | number | null;
  [key: string]: unknown;
}
```
- **Impact**: The `Host` interface models `online?: boolean | number | null` (and test fixtures like host `d` use `online: 1`). However, `selectHosts` applies strict boolean comparison `host.online === true`. If a discovery backend or database layer serializes SQLite boolean flags as integer 1, those hosts are filtered out when the "Available" filter is toggled.
- **Fix**: Check `Boolean(host.online)` or `host.online === true || host.online === 1`.
- **Confidence**: high

## Non-findings checked
- WebGL texture allocation reuse: `renderer.ts` reuses textures across frames via `texSubImage2D`, allocating via `texImage2D(..., null)` only upon resolution changes.
- BT.601 full-range shader matrix: the coefficients in `FS_SOURCE_WEBGL2` and `FS_SOURCE_WEBGL1` match full-range JFIF BT.601 without limited-range expansions.
- WebGL1 chroma sampling: `FS_SOURCE_WEBGL1` correctly samples `.ra` for `gl.LUMINANCE_ALPHA` and uses `.r` (U) and `.g` (V) from the resulting `vec2`.
- Connection teardown deduplication: concurrent invocations of `cancel()`, `disconnect()`, and `retryCleanup()` share the identical in-flight `pendingCleanup` promise without double-cleaning.
- Generation guarding on connection stats and frame completion: `isCurrent(token)` reliably drops stale stats responses and stale frame completions from superseded sessions.
- Port and PIN format validation: `validateConnection` strictly enforces numeric port ranges (1-65535), rejects leading zeros or floats on ports, and validates exactly 8 ASCII digits on non-empty PINs without stripping leading zeros.
- Held input tracking release ordering: `releaseEvents` flushes mouse button releases before key release events, matching the backend's `InputStateTracker::release_all` contract.
- Authoritative command registry completeness: all 22 backend commands registered in `src-tauri/src/lib.rs` are present in `MAHO_COMMANDS`.
- Storage key scoping in `library.ts`: favorites storage isolates records under `mahord.favorites.v1` and sanitizes stored IP strings against case differences and leading/trailing whitespace.
- Static draw buffer reuse: quad position and texture coordinate VBOs are created once on renderer initialization and reused across all render calls.
