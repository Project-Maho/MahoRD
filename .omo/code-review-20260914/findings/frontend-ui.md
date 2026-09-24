# Lane: frontend-ui

## Scope reviewed

All scoped files were read in full:

| File | Lines read |
| --- | ---: |
| `clients/rust/tauri-shell/src/app/App.tsx` | 1-83 (83) |
| `clients/rust/tauri-shell/src/features/host/ThisComputerCard.tsx` | 1-97 (97) |
| `clients/rust/tauri-shell/src/features/library/ComputersPage.tsx` | 1-510 (510) |
| `clients/rust/tauri-shell/src/features/library/DirectConnect.tsx` | 1-119 (119) |
| `clients/rust/tauri-shell/src/features/library/HostGrid.test.tsx` | 1-341 (341) |
| `clients/rust/tauri-shell/src/features/library/HostGrid.tsx` | 1-147 (147) |
| `clients/rust/tauri-shell/src/features/library/LibraryToolbar.tsx` | 1-87 (87) |
| `clients/rust/tauri-shell/src/features/library/SavedCredentials.tsx` | 1-131 (131) |
| `clients/rust/tauri-shell/src/features/session/SessionCanvas.test.tsx` | 1-402 (402) |
| `clients/rust/tauri-shell/src/features/session/SessionCanvas.tsx` | 1-226 (226) |
| `clients/rust/tauri-shell/src/features/session/SessionSettingsPanel.test.tsx` | 1-243 (243) |
| `clients/rust/tauri-shell/src/features/session/SessionSettingsPanel.tsx` | 1-578 (578) |
| `clients/rust/tauri-shell/src/features/session/SessionView.test.tsx` | 1-655 (655) |
| `clients/rust/tauri-shell/src/features/session/SessionView.tsx` | 1-443 (443) |
| `clients/rust/tauri-shell/src/features/shell/Sidebar.tsx` | 1-64 (64) |
| `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts` | 1-546 (546) |
| `clients/rust/tauri-shell/src/styles/globals.css` | 1-93 (93) |
| `clients/rust/tauri-shell/index.html` | 1-12 (12) |

Directly imported implementations consulted:

| File | Lines read |
| --- | ---: |
| `clients/rust/tauri-shell/src/lib/renderer.ts` | 1-386 (386) |
| `clients/rust/tauri-shell/src/lib/overlay.ts` | 1-260 (260) |
| `clients/rust/tauri-shell/src/lib/connection.ts` | 50-320 (270) |
| `clients/rust/tauri-shell/src/lib/ipc.ts` | 1-310 (310) |
| `clients/rust/tauri-shell/src-tauri/src/lib.rs` | 72-95, 796-905, 1775-1825 (182) |
| `clients/rust/maho-host/src/inject_macos.rs` | 50-265 (215) |
| `clients/rust/maho-host/src/inject_windows.rs` | 115-180 (65) |

This is a static, read-only review. The verified build and test suite baseline (`cargo check` and `bun test`) was accepted without rerunning.

## Findings

### [P1] Hardcoded canvas dimensions in JSX corrupt WebGL buffer, mouse mapping, and remote cursor tracking on every re-render
- **Location**: `clients/rust/tauri-shell/src/features/session/SessionCanvas.tsx:188` (secondary: `clients/rust/tauri-shell/src/features/session/SessionView.tsx:117`, `clients/rust/tauri-shell/src/lib/renderer.ts:293`)
- **Evidence**:
```tsx
      <canvas
        id="video-canvas"
        ref={canvasRef}
        width={3840}
        height={1600}
        tabIndex={0}
        aria-label="Remote desktop video; focus to send remote input"
        className="block max-w-full max-h-full object-contain pointer-events-auto"
```
```ts
    if (lastRenderWidth !== width || lastRenderHeight !== height) {
      canvas.width = width;
      canvas.height = height;
      gl.viewport(0, 0, width, height);
      lastRenderWidth = width;
      lastRenderHeight = height;
```
```tsx
      if (elapsed >= 1000) {
        setFps(Math.round((frameCountRef.current * 1000) / elapsed));
        frameCountRef.current = 0;
        lastFpsCalcTimeRef.current = now;
      }
```
- **Impact**:
When incoming video frames arrive, `renderer.render()` dynamically assigns `canvas.width` and `canvas.height` to the remote stream resolution (e.g. 1920x1080) and sets closure variables `lastRenderWidth = 1920, lastRenderHeight = 1080`.
Every 1000ms, `SessionView` triggers a state update via `setFps(...)` (and every 500ms via `connection.refreshStats()`). Because `SessionCanvas` is not memoized, it re-renders. During virtual DOM diffing, React reconciles `<canvas width={3840} height={1600}>` against the DOM element (currently 1920x1080) and overwrites `canvas.width = 3840` and `canvas.height = 1600`.
In the browser WebGL implementation:
1. Resetting `canvas.width` / `height` clears the WebGL drawing buffer to transparent black.
2. On subsequent frames, `renderer.ts` evaluates `lastRenderWidth !== width` (1920 !== 1920), which is `false`. The renderer never resets `canvas.width` back to 1920.
3. The WebGL context viewport remains restricted to (0, 0, 1920, 1080) inside a 3840x1600 canvas buffer, squishing the remote video into the lower-left corner.
4. Both `useRemoteInput.ts:184` (`canvasWidth = canvas.width || 1920`) and `SessionCanvas.tsx:32` (`updateRemoteCursor`) calculate the aspect ratio as `3840 / 1600 = 2.4` instead of `1920 / 1080 = 1.7777`. All client mouse clicks and remote cursor overlays are scaled by 2.4 / 1.777 = 1.35x, causing clicks and cursor positions to severely drift from their intended targets.
- **Fix**: Remove the static `width={3840}` and `height={1600}` attributes from `<canvas>` in `SessionCanvas.tsx`. In `renderer.ts`, compare `canvas.width !== width || canvas.height !== height` directly rather than relying solely on closure variables. Wrap `SessionCanvas` in `React.memo` so telemetry ticks in `SessionView` do not trigger re-renders of the video rendering tree.
- **Confidence**: high

### [P1] Triplicated height-priority styles break text selection globally and introduce layout overflow
- **Location**: `clients/rust/tauri-shell/index.html:2` (secondary: `clients/rust/tauri-shell/index.html:8`, `clients/rust/tauri-shell/src/styles/globals.css:79`, `clients/rust/tauri-shell/src/features/session/SessionView.tsx:212`, `clients/rust/tauri-shell/src/app/App.tsx:60`)
- **Evidence**:
```html
<html lang="en" style="width: 100%; height: 100%; margin: 0; padding: 0; overflow: hidden;">
```
```html
  <body style="width: 100%; height: 100%; margin: 0; padding: 0; overflow: hidden; user-select: none;">
    <div id="root" style="width: 100%; height: 100%; margin: 0; padding: 0; overflow: hidden;"></div>
```
```css
  html,
  body,
  #root {
    width: 100%;
    height: 100%;
    margin: 0;
    padding: 0;
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
  }
```
```tsx
      className="fixed inset-0 w-full h-full overflow-hidden bg-black select-none z-[1000]"
      style={{
        position: "fixed",
        inset: 0,
        width: "100vw",
        height: "100vh",
        overflow: "hidden",
      }}
```
- **Impact**:
1. Specificity shadow: The inline styles on `<html>`, `<body>`, and `<div id="root">` in `index.html` have specificity `1,0,0,0`. They completely override `@layer base` in `globals.css` (which has lowest cascade layer priority). Changes made to `globals.css` base rules have no effect.
2. Global text selection breakage: By applying `user-select: none` to `body` globally in both `index.html` and `globals.css`, text selection is disabled across the entire dashboard, including `ComputersPage`, `ThisComputerCard`, `SavedCredentials`, and `HostGrid`. Users cannot select or copy computer hostnames, IP addresses, pairing IDs, or diagnostic messages. Furthermore, the clipboard fallback in `ComputersPage.tsx:219` (`ta.select()`) fails in WebKit because `<textarea>` inherits `user-select: none`.
3. Sizing conflicts: `SessionView.tsx` duplicates Tailwind classes (`w-full h-full`) with inline styles (`width: 100vw; height: 100vh`). In browsers and webviews with standard or non-overlay scrollbars (Windows WebView2 and Linux WebKitGTK), `100vw` includes the scrollbar track and exceeds `100%` of client width. This forces horizontal layout overflow, directly violating the requirement that no scrollbar ever appears.
- **Fix**: Remove all inline styles from `index.html` on `<html>`, `<body>`, and `<div id="root">`. Remove `user-select: none` and `overflow: hidden` from `globals.css` on `body` and `#root`. Scope `select-none` and `overflow-hidden` strictly to the session container (`SessionView` / active session wrapper in `App.tsx`). In `SessionView.tsx`, remove the inline `style` object and let `fixed inset-0 w-full h-full` control dimensions without `100vw` / `100vh`.
- **Confidence**: high

### [P1] Missing `e.preventDefault()` on forwarded keys breaks session focus on Tab and triggers webview navigation
- **Location**: `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:390` (secondary: `clients/rust/tauri-shell/src/lib/overlay.ts:114`)
- **Evidence**:
```ts
    // lines 1434-1489 window keydown/keyup
    const handleKeyDown = (e: KeyboardEvent) => {
      const target = e.target as { tagName?: unknown; id?: unknown } | null;
      if (!shouldForwardKeyboardEvent({ target })) {
        if (e.key === "Escape") {
          optionsRef.current.onEscape?.();
        }
        return;
      }
      if (!isConnectedRef.current) return;
      if (e.repeat) return;

      let mod = 0;
      if (e.shiftKey) mod |= 1;
      if (e.ctrlKey) mod |= 2;
      if (e.altKey) mod |= 4;
      if (e.metaKey) mod |= 8;

      const keyCode =
        (e.keyCode !== undefined ? e.keyCode : (e.which ?? 0)) || 0;
      heldInputsRef.current.keyDown(keyCode, mod);

      const viewWidth =
        canvas?.clientWidth || lastPointerRef.current.viewWidth || 1280;
      const viewHeight =
        canvas?.clientHeight || lastPointerRef.current.viewHeight || 800;

      sendInput({
        event_type: "KeyDown",
        key_code: keyCode,
        modifiers: mod,
        view_width: viewWidth,
        view_height: viewHeight,
      }).catch((err) => {
        console.error("send_input keydown error:", err);
      });
    };
```
- **Impact**:
1. When a key is destined for the remote host (`shouldForwardKeyboardEvent(target)` is `true`), `handleKeyDown` and `handleKeyUp` never invoke `e.preventDefault()`.
2. Pressing `Tab` triggers default browser focus traversal, moving DOM focus from the remote canvas to the floating overlay launcher buttons (`#btn-home`, `#btn-expand`). Once focus shifts to `#btn-home`, subsequent key events have `target.id === 'btn-home'`. `shouldForwardKeyboardEvent` evaluates to `false`, causing all subsequent typing, arrows, and shortcuts to be permanently blocked until the user re-focuses the canvas with a mouse click.
3. Common remote desktop keys trigger default webview behaviors: `Space` and arrow keys scroll the container, `F5` / `Ctrl+R` reloads the webview (terminating the active session), and `Backspace` can trigger browser backward navigation.
- **Fix**: Call `e.preventDefault()` inside `handleKeyDown` and `handleKeyUp` whenever `shouldForwardKeyboardEvent({ target })` is true and `isConnectedRef.current` is active.
- **Confidence**: high

### [P1] Unconditional `e.repeat` filtering disables typematic key repeat on remote host
- **Location**: `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:401` (secondary: `clients/rust/maho-host/src/inject_macos.rs:241`, `clients/rust/maho-host/src/inject_windows.rs:158`)
- **Evidence**:
```ts
      if (!isConnectedRef.current) return;
      if (e.repeat) return;

      let mod = 0;
```
```rust
            InputEventType::KeyDown => {
                self.active_keys.borrow_mut().insert(event.key_code);
                CGEvent::new_keyboard_event(source()?, event.key_code, true)
            }
```
```rust
                    inputs.push(key_input(vk, is_up));
```
- **Impact**:
1. When a user holds down a key (such as Backspace, Delete, Arrow keys, Enter, or letters), the client webview generates an initial `keydown` with `repeat: false`, followed by a continuous stream of repeated `keydown` events with `repeat: true`.
2. `useRemoteInput.ts` drops every event where `e.repeat` is true.
3. On both macOS (`CGEvent`) and Windows (`SendInput`), software-injected `KeyDown` events do not engage the OS hardware keyboard subsystem typematic repeat timer.
4. Holding down Backspace deletes only a single character, holding down an arrow key moves the cursor by only a single cell, and holding down any alphanumeric key produces only a single character on the remote machine.
- **Fix**: Do not return early on `e.repeat` for forwarded keys. Update `handleKeyDown` to forward repeated `KeyDown` events to `sendInput`, while ensuring `heldInputsRef.current.keyDown` records only the initial key-down to avoid redundant tracking.
- **Confidence**: high

### [P1] Missing pointer capture and viewport-scoped mousemove drops active drag events outside canvas
- **Location**: `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:466` (secondary: `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:478`)
- **Evidence**:
```ts
    if (viewport) {
      viewport.addEventListener("mousedown", handleMouseDown as EventListener);
      viewport.addEventListener("mousemove", handleMouseMove as EventListener);
      viewport.addEventListener("contextmenu", handleContextMenu as EventListener);
      viewport.addEventListener("wheel", handleWheel as EventListener, {
        passive: false,
      });
    }
```
```ts
    if (typeof window !== "undefined") {
      window.addEventListener("mouseup", handleWindowMouseUp as EventListener);
      window.addEventListener("keydown", handleKeyDown as EventListener);
      window.addEventListener("keyup", handleKeyUp as EventListener);
      window.addEventListener("blur", handleBlur as EventListener);
    }
```
- **Impact**:
1. `mousemove` is listened only on `viewport`, whereas `mouseup` is listened on `window`. Neither `setPointerCapture` nor window-level drag tracking is implemented.
2. When a user initiates a drag (such as dragging a window, selecting text, or dragging a file) and the cursor exits the bounds of `#viewport`, passes over `#session-overlay`, or crosses a display boundary, `#viewport` stops receiving `mousemove` events.
3. The remote host receives no further movement updates during the out-of-bounds drag.
4. When the user releases the mouse button on `window`, `handleWindowMouseUp` sends `LeftMouseUp` at the last recorded position, causing the dragged element to drop or teleport unpredictably.
- **Fix**: Use Pointer Events with `e.currentTarget.setPointerCapture(e.pointerId)` on pointer down and `releasePointerCapture(e.pointerId)` on pointer up, or attach `mousemove` to `window` while a mouse button is actively held down.
- **Confidence**: high

### [P2] Effect dependency omissions cause stale element references and orphan held inputs on disconnect
- **Location**: `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:69` (secondary: `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:133`, `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:541`)
- **Evidence**:
```ts
  const isConnected = options.isConnected ?? true;
  const isConnectedRef = useRef(isConnected);
  useEffect(() => {
    isConnectedRef.current = isConnected;
  }, [isConnected]);
```
```ts
  useEffect(() => {
    const viewport = resolveElement<HTMLElement>(
      optionsRef.current.viewport,
      ["viewport"]
    );
    const canvas = resolveElement<HTMLCanvasElement>(
      optionsRef.current.canvas,
      ["video-canvas", "screen-canvas"]
    );
    const overlay = resolveElement<HTMLElement>(
      optionsRef.current.overlay,
      ["session-overlay"]
    );
```
```ts
      // Teardown: flush motion and release all held inputs so nothing stays stuck on the host
      flushPointerMotion();
      releaseAll().catch(() => {});
    };
  }, [releaseAll, flushPointerMotion]);
```
- **Impact**:
1. The listener setup `useEffect` has dependency array `[releaseAll, flushPointerMotion]`. If a caller passes `options.canvas` or `options.viewport` as React refs whose `.current` properties are populated after mount, or if the canvas DOM element is remounted, the hook never re-resolves the elements and listeners are never attached.
2. When `isConnected` transitions from `true` to `false` (e.g. streaming pauses, reconnects, or encounters a socket drop), the main effect cleanup does not run because `isConnected` is omitted from dependencies. Any modifier keys or mouse buttons held down at that instant remain stored in `heldInputsRef.current` without being released, and subsequent `handleKeyUp` events return early without clearing them. When reconnecting, held state remains dirty.
- **Fix**: When `isConnected` transitions to `false` in its synchronization effect, call `releaseAll()` to release and flush held keys. For element refs, resolve elements reactively or accept ref dependencies.
- **Confidence**: high

### [P2] `handleWheel` ignores letterbox offset and strips keyboard modifiers
- **Location**: `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:365`
- **Evidence**:
```ts
    // lines 1413-1432 viewport wheel
    const handleWheel = (e: WheelEvent) => {
      if (!isConnectedRef.current) return;
      e.preventDefault?.();
      const rect = canvas?.getBoundingClientRect?.() ?? {
        left: 0,
        top: 0,
        width: 1280,
        height: 800,
      };
      const width = rect.width > 0 ? rect.width : 1280;
      const height = rect.height > 0 ? rect.height : 800;
      const rawX = Math.max(0, Math.min(e.clientX - rect.left, width));
      const rawY = Math.max(0, Math.min(e.clientY - rect.top, height));

      sendInput({
        event_type: "ScrollWheel",
        x: rawX,
        y: rawY,
        view_width: width,
        view_height: height,
        modifiers: 0,
        scroll_dx: e.deltaX,
        scroll_dy: e.deltaY,
      }).catch((err) => {
        console.error("send_input wheel error:", err);
      });
    };
```
- **Impact**:
1. In letterboxed or pillarboxed canvas configurations, `handleWheel` maps `rawX` and `rawY` against the raw `rect.width` and `rect.height` without computing or subtracting `offsetX` and `offsetY`. Unlike `sendPointerEvent`, mouse wheel events are injected at coordinates offset from the active video content.
2. `modifiers: 0` is hardcoded. Holding Ctrl while scrolling (Ctrl+Wheel to zoom) or holding Shift (Shift+Wheel to pan horizontally) strips all modifier keys from the scroll payload sent to the remote host.
- **Fix**: Apply the same aspect ratio letterbox projection math (`offsetX`, `offsetY`, `displayW`, `displayH`) used in `sendPointerEvent`, and populate `modifiers` from `e.shiftKey`, `e.ctrlKey`, `e.altKey`, and `e.metaKey`.
- **Confidence**: high

### [P2] Unreferenced `SessionSettingsPanel.tsx` monkey-patches `document.createElement` in production module scope
- **Location**: `clients/rust/tauri-shell/src/features/session/SessionSettingsPanel.tsx:147`
- **Evidence**:
```ts
// Ensure minimal fake DOM environments don't crash when React DOM mounts <select> or applies CSS variables
if (typeof document !== "undefined" && typeof document.createElement === "function") {
  const origCreateElement = document.createElement.bind(document);
  document.createElement = function (tagName: string, options?: any) {
    const el = origCreateElement(tagName, options);
    if (el) {
      if (typeof (el as any).querySelectorAll !== "function") {
        (el as any).querySelectorAll = function (selector: string) {
          const matches: any[] = [];
          function search(node: any) {
```
- **Impact**:
1. `SessionSettingsPanel.tsx` is completely unreferenced by production code (not imported in `App.tsx`, `SessionView.tsx`, or any other consumer). Its 578 lines of UI logic are dead code.
2. Despite being unreferenced, importing it in test suites or bundling it in client chunks executes top-level code that monkey-patches `document.createElement` on the real browser `document`, wrapping every created element in polyfill proxies (`querySelectorAll`, `closest`, `style.setProperty`).
- **Fix**: Move fake DOM test polyfills into dedicated test setup files (`test/setup.ts`). Wire `SessionSettingsPanel` into `SessionView` if audio/quality settings controls are intended, or remove the dead file.
- **Confidence**: high

### [P3] Inverted canvas ID guard prevents focusing canvas on mouse down
- **Location**: `clients/rust/tauri-shell/src/features/session/useRemoteInput.ts:291` (secondary: `clients/rust/tauri-shell/src/features/session/SessionCanvas.tsx:187`)
- **Evidence**:
```ts
    const handleMouseDown = (e: MouseEvent) => {
      if (!isConnectedRef.current) return;
      viewport?.focus?.();
      if (canvas && canvas.id !== "video-canvas") {
        canvas.focus?.();
      }
```
```tsx
      <canvas
        id="video-canvas"
        ref={canvasRef}
        width={3840}
        height={1600}
        tabIndex={0}
        aria-label="Remote desktop video; focus to send remote input"
```
- **Impact**:
`<canvas id="video-canvas">` is configured with `tabIndex={0}` and `aria-label="Remote desktop video; focus to send remote input"`. However, `handleMouseDown` specifically checks `canvas.id !== "video-canvas"`, skipping `canvas.focus()` whenever the canvas ID is `"video-canvas"`. Instead, it only focuses `#viewport`.
- **Fix**: Change the condition to `if (canvas && canvas.id === "video-canvas") canvas.focus?.();` or simply call `canvas?.focus?.()`.
- **Confidence**: high

### [P3] Duplicate frame parsing and buffer slice allocations on the 60fps render loop
- **Location**: `clients/rust/tauri-shell/src/features/session/SessionCanvas.tsx:143` (secondary: `clients/rust/tauri-shell/src/lib/renderer.ts:373`)
- **Evidence**:
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
```
```ts
    render(buf: ArrayBuffer): void {
      if (disposed) return;
      const frame = parseFrame(buf);
      if (!frame) return;
      renderNv12(frame.width, frame.height, frame.y, frame.uv);
    },
```
- **Impact**:
For every video frame at 60 FPS, `parseFrame(buf)` is called twice: once inside `renderer.render(buf)` and once in `SessionCanvas.tsx`. Each call instantiates a `DataView` and two `Uint8Array` slice views (`y` and `uv`), creating redundant garbage collection overhead on the hot rendering path.
- **Fix**: Parse the frame once per loop tick in `SessionCanvas` and pass the parsed frame directly to the renderer, or return the parsed frame from `render()`.
- **Confidence**: high

## Non-findings checked

- Single-mount WebGL lifecycle: `createRenderer` in `SessionCanvas.tsx` runs inside an effect with dependency `[]`, and correctly invokes `dispose()` on unmount without re-creating WebGL contexts on prop changes.
- Frame loop invalidation: Generation counter (`generationRef`) correctly invalidates in-flight asynchronous frame polling loops when `active` toggles to false, preventing stale frame rendering.
- Fullscreen synchronization: Overlay state in `SessionView.tsx` correctly listens to `fullscreenchange` events on `document` and synchronizes the expanded overlay button state.
- Numeric PIN validation: `DirectConnect.tsx` enforces `inputMode="numeric"` and validates 8-digit ASCII PIN format before dispatching connection requests.
- Relative pointer coalescing: `useRemoteInput.ts` correctly coalesces relative pointer movements (`RelativeMove`) into cumulative `scroll_dx` and `scroll_dy` during active pointer lock.
- Sidebar view switching: `Sidebar.tsx` correctly applies `aria-current` and handles responsive collapse for mobile/narrow window sizes without layout distortion.
- Host sharing toggling: `ThisComputerCard.tsx` properly reflects host server running status and handles busy states during start/stop operations.
