# Windows login-screen QA artifacts

Evidence and reusable scripts from the MahoRDHost work. See
`docs/windows-login-screen.md` for the operator guide.

- `evidence-secure-desktop.png` — a live frame captured from `maho-win` while the
  UAC consent desktop (`WinSta0\Winlogon`) was on screen, decoded from the agent
  screenshot endpoint. This is the criterion-4 proof.
- `rebuild-hint.ps1` — sync-free rebuild + reinstall of the service, printing
  `BUILD_EXIT`, `sc query STATE`, the process/session pair and the desktop hint.
- `install-service.ps1` — stop the legacy scheduled task, install the service,
  and report its state.
- `migrate-store.ps1` — copy pairing records from `%APPDATA%` to `%ProgramData%`.
- `acl-check.ps1` / `file-acl.ps1` — audit the ACL on the store directory and on
  `host-authorizations.json`; expect SYSTEM and Administrators only.
- `verify-lock.ps1` — tail the supervisor decisions from `service.log`.

Run them on the host with `powershell -NoProfile -ExecutionPolicy Bypass -File`;
inline quoting through SSH gets mangled.
