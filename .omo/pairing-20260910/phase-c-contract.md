# Phase C integration contract

This coordinator contract fills the type/API gaps in the earlier illustrative contract. Existing lead amendments, role separation, v3 framing and completed A/B behavior remain binding.

## Credential metadata

Keep `erd_app::PairingRecord` and `erd_host::PairingRecord` separately named and scoped. Only the client record gains metadata. Its existing `id`, `name`, key encoding, `addedAt` conversion, filesystem format and Keychain service remain compatible.

Add a shared `PairingEndpoint` with camelCase JSON fields:

```text
host: String
tcp_port: u16
udp_port: u16
```

The host string is the transport host, not a display name; preserve IPv6 zone information. These concrete ports come from the validated connection configuration. Loopback/direct endpoints are valid metadata; do not apply LAN-publication filtering to saved credentials.

Client record additions:

```text
last_endpoint: Option<PairingEndpoint>  // default None; omit when absent
endpoint_aliases: Vec<PairingEndpoint> // default empty; omit when empty
```

Keep the current array-of-records storage schema with additive optional fields; no new database or envelope version. Old records deserialize without metadata. Adding Rust fields requires updating client-record literal constructors, not similarly named host-record constructors.

Expose one shared key-free `PairingSummary` for both shells: `id`, `host_name`, `added_at_unix_ms`, and optional endpoint metadata. Existing legacy responses still omit absent metadata. The previously always-absent `lastEndpoint` now carries the endpoint object when known. The desktop may re-export this shared type at its existing public path. Never serialize a storage record into IPC.

Provide an exact-ID, metadata-only store update, for example:

```text
remember_endpoint(id, expected_key, endpoint) -> Result<bool, PairingStoreError>
```

It preserves the stored key/name/time, updates only a record whose ID and key still match, deduplicates prior endpoint hints, and returns false rather than recreating a missing/forgotten record. Use the existing persistence backend. Do not claim cross-process transactional guarantees the store does not implement.

Desktop/iOS callers invoke it after successful authentication in the stored/new-pairing flow. Do not add a mandatory store read/write to the core one-off `connect_with_pairing` path used by explicit-PSK CLI/QA clients. Such calls must not start persisting credentials or fail because an unrelated default store is unavailable.

## Explicit selection

Keep flat Tauri arguments. The new optional field is Rust `pairing_id`, JS `pairingId`, on both shells.

- Explicit PIN means intentional fresh pairing, as in the existing contract.
- Without PIN, a supplied `pairingId` loads exactly that ID.
- Missing/unknown ID without PIN yields a structured `pairing-required` error before starting transport. Never guess from host, name, prefix, discovered name or last endpoint.
- The UI presents saved credentials separately from untrusted discovered endpoints and sends either a PIN or a chosen ID. Same-name credentials must be distinguishable by ID/endpoint metadata.
- Last endpoints and aliases are transport hints only; they never establish trust or silently select a key.
- Persist endpoint metadata only after successful authentication. Persistence errors must be surfaced without leaving a falsely connected UI or leaking a provisional session.

## Shared IPC errors

Use typed code/stage enums with serialization matching the earlier JSON strings, plus an `IpcError` carrying `code`, `message`, `stage`, and `retryable`. Keep existing native error types; classify them before rendering a human message. No blanket conversion of arbitrary strings into credential rejection.

Retain the documented codes: pairing-required, pairing-denied, pairing-locked-out, pairing-disabled, credential-rejected, invalid-pin, consent-timeout, handshake-timeout, remote-closed, network-unreachable, cleanup-failed, cancelled.

Add two necessary non-misleading fallbacks:

- `connection-failed` for otherwise unclassified connection/TLS failures.
- `incompatible-peer` for the typed missing authenticated-registration capability error introduced by R3.

Stages remain client, connect, preauth, tls-psk, handshake, runtime, cleanup. Only classify a credential as rejected when a typed signal supports that conclusion. No prose-substring decisions; never include keys or entered PINs in IPC error data. Tests assert machine fields/redaction, not exact human wording.

## Ownership and gates

The shared-contract producer owns client metadata, shared summary/error types and necessary client-record literal adaptation. It does not change identity selection, discovery policy, UI or lifecycle behavior.

The scoped-discovery producer owns net/Apple scope preservation without changing the public discovered-host JSON shape. Keep scoped host strings intact and preserve IPv4 preference.

Desktop and iOS producers depend on both common producers before editing their own command/UI paths. Desktop integrates the already verified but uncommitted authentication helper/R11 tests. Existing unsafe name-based trust expectations must become stronger R8 regressions, not be silently removed; unrelated discovery improvements remain.

The new concurrent host-management policy is explicitly excluded from these nodes pending the separately recorded ownership/policy resolution. Do not delete its API or change its controls opportunistically.

All new regression names and RED invocations must be pinned before action. Rust compilation stays on Omarchy, except physical-iOS target build/signing on Mac. No emulators, publishing or permanent deployment.
