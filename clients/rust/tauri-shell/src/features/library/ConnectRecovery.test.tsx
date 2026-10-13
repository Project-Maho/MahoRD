import { test, expect } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { ConnectRecovery, CONNECT_RECOVERY } from "./ConnectRecovery"

test("every existing backend error has a recovery mapping and retry control", () => {
  const codes = ["pairing-required", "pairing-denied", "pairing-locked-out", "pairing-disabled", "credential-rejected", "invalid-pin", "consent-timeout", "handshake-timeout", "remote-closed", "network-unreachable", "cleanup-failed", "cancelled", "connection-failed", "incompatible-peer"]
  expect(Object.keys(CONNECT_RECOVERY).sort()).toEqual(codes.sort())
  for (const code of codes) {
    const html = renderToStaticMarkup(<ConnectRecovery code={code} error="backend detail" onRetry={() => {}} />)
    expect(html).toContain('data-action="retry-connect"')
  }
})

test("PIN recovery is offered only for unpaired attempts", () => {
  const props = { code: "pairing-required", error: "missing", onRetry: () => {}, onPin: () => {} }
  expect(renderToStaticMarkup(<ConnectRecovery {...props} />)).toContain(">Enter PIN</button>")
  expect(renderToStaticMarkup(<ConnectRecovery {...props} paired />)).not.toContain(">Enter PIN</button>")
})

test("credential recovery requires an explicit forget action and cleanup has its own retry", () => {
  expect(renderToStaticMarkup(<ConnectRecovery error="rejected" code="credential-rejected" paired onRetry={() => {}} onForget={() => {}} />)).toContain(">Forget pairing…</button>")
  expect(renderToStaticMarkup(<ConnectRecovery error="cleanup" code="cleanup-failed" onRetry={() => {}} onCleanup={() => {}} />)).toContain(">Retry cleanup</button>")
})
