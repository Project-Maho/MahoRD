# R7 RED evidence

Actual iOS page sources; only Tauri IPC replaced by a deterministic deferred-connect bridge. Product code unchanged.

Tool: existing playwright-core with /Applications/Google Chrome.app/Contents/MacOS/Google Chrome, isolated headless profile. Bun WebView fallback reason: host process failed to spawn after a cancelled kernel.

Actions: page.goto(local ephemeral server); page.fill('#connect-host', '192.0.2.10'); subscribe qa.arrival; page.click('#btn-connect-submit'); await bounded qa.arrival.

Expected: Cancel is visible/reachable. Actual RED:
```json
[
  {
    "viewport": "mobile",
    "issued": true,
    "rects": 0,
    "hiddenAncestor": "view-session",
    "disabled": false,
    "active": "btn-connect-submit"
  },
  {
    "viewport": "desktop",
    "issued": true,
    "rects": 0,
    "hiddenAncestor": "view-session",
    "disabled": false,
    "active": ""
  }
]
```

Screenshots: r7-red-mobile.png (430x932), r7-red-desktop.png (1280x800).
Both connect calls issued; Cancel had zero client rects under hidden view-session. No network authentication was simulated as success.

Cleanup: all pages and owned Chrome browser closed; temporary profile removed by Playwright browser.close; Bun server 63862 stopped. Earlier failed server 63440 was also stopped.
Screenshot files exist, but lead read tool reported current model does not support images; visual pixel verdict remains unverified pending image-capable check. DOM visibility RED is directly observed.
