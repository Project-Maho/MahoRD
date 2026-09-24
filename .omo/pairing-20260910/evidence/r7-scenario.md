# R7 real page scenario

Source: actual `clients/rust/ios-shell/ui/` files, served without changing UI code. Only the Tauri IPC boundary is replaced by a deterministic bridge. No claim of native authentication.

Browser: `new Bun.WebView({width: 430, height: 932})`, then repeat at 1280x800.
Actions: navigate local ephemeral server; `view.click('#connect-host')`; `view.type('#connect-host', '192.0.2.10')`; `view.click('#btn-connect-submit')`.
The bridge's connect promise remains pending. Subscribe to the bridge command before clicking.
PASS: `#btn-cancel-connect` has nonzero client rects, no hidden ancestor, is enabled, and is the active element or keyboard reachable while background is inert.
Expected RED before implementation: connect is issued but Cancel has a hidden `#view-session` ancestor and no client rects.
Artifacts: `r7-red-mobile.png`, `r7-red-desktop.png`, and `r7-red-results.md`.
Cleanup: close each WebView and stop the Bun server after capture.
