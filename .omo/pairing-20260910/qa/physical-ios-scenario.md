# Physical iOS persistence/session QA preparation

This is a planned coordinator scenario, not a completed device test.

## Observed readiness

`xcrun devicectl list devices` completed successfully and reported:

- iPhone 16 Plus, CoreDevice `F1C581E0-A54E-5E85-8013-4F02DF80F98B`: available, paired.
- iPhone 12 Pro, CoreDevice `DBB7A424-6196-5A4D-88EA-CE441C4B8132`: available, paired.

This does not prove the device is unlocked, the development image is mounted, signing succeeds, or the current implementation is installed.

The checked-in app identifier is `com.eclipticrd.ios`. Generated entitlements are currently an empty dictionary; the existing export options use method `debugging`. The main user app and its data must remain intact.

## Existing provisioning boundary

Debug builds inspect `Documents/erd-device-qa.json`. The current input carries `host` and an optional client `PairingRecord`. The old implementation writes two stores, ignores persistence errors, removes the input, and returns only a host/auto-connect hint.

That old response is insufficient for explicit-ID reconnect proof. The iOS phase-C worker has been authorized to minimally adapt this existing path to the production client/Keychain store, checked import results, and an explicit selected ID plus transport endpoint. It must not return keys/PINs or add another credential store/remote control backdoor.

## Planned coordinator sequence

1. Review the final R9 implementation and its exact debug provisioning response. Read the actual build/signing commands and confirm the selected physical device is ready before running them.
2. Prefer an isolated QA bundle identifier/build copy if existing provisioning permits it, rather than overwriting the user's installed app. Do not change the checked-in production identifier or use a simulator.
3. Start an owned, temporary host that supports the R3 capability, with an isolated authorization store and observed reachable TCP/UDP ports. Existing production hosts remain untouched.
4. Seed a unique test credential in the app's actual client Keychain backend. Its friendly name must differ from the numeric host address, so name-based lookup cannot satisfy the test. Store the observed endpoint metadata and use an explicit pairing ID.
5. Verify the actual key-free `list_pairings` response contains that ID and endpoint after import. Do not infer persistence from a log message.
6. Drive the real app connection flow in saved-credential mode without entering a PIN. Capture UI actions, native connection/frame evidence and a physical-device screenshot.
7. Terminate the QA app through the device tools, confirm the provisioning input was consumed, and relaunch without another import. Select the same saved credential explicitly, connect without PIN, and capture a second session/frame result.
8. Exercise missing/unknown ID and remote-close/cancel scenarios only once their C/D implementations and exact expected outcomes are pinned. Do not call a browser fixture or compilation a physical runtime pass.
9. Disconnect and reap the owned host/client resources; remove only the QA app, seeded records and temporary build/provisioning artifacts owned by this run. Preserve the original app, keys and production hosts.

Record exact observed commands, identifiers, ports, source/binary versions, outcomes and cleanup receipts before accepting R9/device criteria. Signing, import, connection and lifecycle failures remain failures; report them rather than falling back to a simulator, a default PIN or a fake success.
