# Lane: net-crypto

## Scope reviewed

All lines, including tests, were read in these files:

- `clients/rust/maho-net/src/tls_psk.rs`: 972 lines.
- `clients/rust/maho-net/src/udp_gcm.rs`: 657 lines.
- `clients/rust/maho-net/src/signaling.rs`: 308 lines.
- `clients/rust/maho-net/src/stun.rs`: 250 lines.
- `clients/rust/maho-net/src/lib.rs`: 17 lines.
- Directly imported framing/header dependencies: `clients/rust/maho-proto/src/lib.rs`: 33 lines; `clients/rust/maho-proto/src/framing.rs`: 209 lines; `clients/rust/maho-proto/src/packet.rs`: 116 lines.
- Directly included test modules: `clients/rust/maho-net/src/nonblocking_write_tests.rs`: 218 lines; `clients/rust/test-support/allocations.rs`: 46 lines.

Review boundary: the allowed files do not contain the host admission/auth/consent coordinator or session-salt generator. Consequently, propagation of one `admission_deadline` through TLS, application authentication, and consent, lockout enforcement at admission, and salt freshness across reconnects are **not verified**. No caller outside the permitted scope was read. No source was changed, and neither baseline command was rerun.

## Findings

### [P0] Signaling exposes a cheap offline verifier for the bootstrap PIN
- **Location**: `clients/rust/maho-net/src/signaling.rs:93`; secondary: `clients/rust/maho-net/src/tls_psk.rs:622`.
- **Evidence**:
```rust
        let topic_seed = hkdf_sha256(pin.as_bytes(), SIGNALING_SALT, b"maho/topic", 32);
        let topic = format!("erd3-{}", lower_hex(&topic_seed[..14]));
        let key = hkdf_sha256(pin.as_bytes(), SIGNALING_SALT, b"maho/payload-key", 32);
```
```rust
pub fn bootstrap_psk(pin: &str) -> Result<[u8; 32], TlsPskError> {
    if pin.len() != 8 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TlsPskError::InvalidIdentity);
    }
    let mut stretched = [0_u8; 32];
    pkcs5::pbkdf2_hmac(
        pin.as_bytes(),
        BOOTSTRAP_SALT,
        BOOTSTRAP_STRETCH_ROUNDS,
        MessageDigest::sha256(),
        &mut stretched,
    )?;
    let derived = hkdf_sha256(&stretched, BOOTSTRAP_SALT, b"maho/tls-psk", 32);
```
- **Impact**: The ntfy operator, a compromised signaling service, or anyone who obtains the topic URL can enumerate the 100,000,000 possible eight-digit PINs using only cheap HKDF/HMAC operations and compare the resulting topic. This bypasses the intended 600,000-round guessing cost of the TLS bootstrap derivation and never increments `BootstrapLockout`. Recovering the PIN also reveals the signaling encryption key and lets the attacker derive the bootstrap PSK once and impersonate a PIN-holder while pairing is open. Separate host consent is not proven bypassed. An independent Python RFC 5869 calculation recovered a demonstration PIN from its topic by enumeration without calling PBKDF2.
- **Fix**: Feed the PIN through `bootstrap_psk` or an equally expensive shared PIN stretcher before deriving **both** signaling outputs, retaining distinct topic/payload labels and updating both peers together. This restores the intended per-guess cost; eliminating the offline PIN verifier altogether requires a high-entropy rendezvous secret or a PAKE-based exchange rather than a publicly observable deterministic PIN-derived topic.
- **Confidence**: high

### [P0] Signaling buffers an unlimited remote HTTP body before authentication
- **Location**: `clients/rust/maho-net/src/signaling.rs:145`.
- **Evidence**:
```rust
            if let Ok(Ok(response)) = timeout(remaining, self.http.get(&poll_url).send()).await {
                if let Ok(body) = response.bytes().await {
                    if let Some(candidate) =
                        peer_candidate_from_jsonl(&body, &self.role, &self.payload_key)
                    {
                        return Ok(candidate);
                    }
                }
            }
```
- **Impact**: A malicious or compromised configured ntfy endpoint can send a large or chunked response and make the client retain the entire body before checking any encrypted candidate. There is no application byte limit, so a sufficiently large response can exhaust memory and terminate the process. The endpoint does not need the PIN or a valid GCM tag. A request timeout is not a memory bound; the JSON envelope and base64 decoding subsequently create additional allocations from the same untrusted data.
- **Fix**: Consume response chunks under the absolute exchange deadline and stop with an explicit error once a bounded response budget is exceeded, including when `Content-Length` is absent or false. Bound each JSONL envelope and encoded candidate before allocating/decrypting it. For example, enforce a 256 KiB poll-response limit and a 16 KiB envelope limit, with protocol-appropriate bounded candidate strings, rather than using unrestricted `Response::bytes()`.
- **Confidence**: high

### [P1] A new signaling exchange accepts candidates from an earlier session
- **Location**: `clients/rust/maho-net/src/signaling.rs:137`; secondary: `clients/rust/maho-net/src/signaling.rs:199`.
- **Evidence**:
```rust
        let poll_url = format!("{topic_url}/json?poll=1&since=10m");
```
```rust
    let text = std::str::from_utf8(body).ok()?;
    text.lines().rev().find_map(|line| {
        let envelope = serde_json::from_str::<Envelope>(line).ok()?;
        let payload = decrypt_payload(&envelope.message, key)?;
        let candidate = serde_json::from_slice::<SessionCandidate>(&payload).ok()?;
        (candidate.role != own_role).then_some(candidate)
    })
```
- **Impact**: Topic and key are stable for a PIN, but the only acceptance condition after decryption is a different role. On a retry/reconnect with the same PIN within ten minutes, one side can poll before the other side publishes its new candidate and immediately return the previous session's IP/port. NAT traversal then targets a stale socket instead of waiting for the current peer. A signaling service can also replay captured ciphertext indefinitely because neither the encrypted candidate nor its acceptance binds it to the current exchange; reversing JSONL order does not establish freshness.
- **Fix**: Add an authenticated per-exchange identifier/challenge to the signaling protocol and require the peer's candidate to bind to the current exchange before returning it. Use a request/response challenge exchange so independently starting peers can establish that binding, and stop treating historical messages with only a matching PIN and opposite role as current candidates. An ntfy timestamp or a shorter history query alone is not cryptographic replay protection.
- **Confidence**: high

### [P2] STUN returns a port mapping for a socket that it immediately closes
- **Location**: `clients/rust/maho-net/src/stun.rs:60`.
- **Evidence**:
```rust
    pub async fn fetch_public_address(&self) -> Result<SocketAddr, StunError> {
        let server = resolve_ipv4(&self.server)?;
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .map_err(StunError::Io)?;
        socket.connect(server).await.map_err(StunError::Io)?;

        let mut transaction_id = [0_u8; 12];
        rand_bytes(&mut transaction_id).map_err(StunError::Random)?;
        let request = binding_request(transaction_id);
        socket.send(&request).await.map_err(StunError::Io)?;

        let mut response = [0_u8; 2048];
        let count = timeout(self.request_timeout, socket.recv(&mut response))
            .await
            .map_err(|_| StunError::Timeout)?
            .map_err(StunError::Io)?;
        parse_binding_response(&response[..count], transaction_id)
    }
```
- **Impact**: The returned public port describes the temporary ephemeral socket, which is dropped on return. Using this `SocketAddr` as a NAT-traversal candidate therefore advertises a port with no receiving session socket; a separately bound media socket need not have the same mapping, even with endpoint-independent NAT. IP-only discovery still works. This is an API correctness finding, not a claim that an out-of-scope caller was observed advertising the returned port.
- **Fix**: Perform STUN on the UDP socket that the session retains and will use for peer traffic: accept a borrowed socket and use `send_to`/`recv_from`, validating the STUN source and transaction ID without permanently connecting the socket to the STUN server. Alternatively return ownership of the probed socket with the mapped address. If this API is intentionally only public-IP discovery, return `IpAddr` instead of exposing an unusable transport candidate.
- **Confidence**: high

### [P2] Checked-in crypto vectors disagree with the implemented derivations
- **Location**: `clients/rust/maho-net/src/signaling.rs:273`; secondary: `clients/rust/maho-net/src/udp_gcm.rs:314`, `clients/rust/maho-net/src/tls_psk.rs:821`, and HKDF implementation `clients/rust/maho-net/src/udp_gcm.rs:250`.
- **Evidence**:
```rust
        assert_eq!(first.topic(), "erd3-a1ac9ca40211ed7a122586e77c18");
```
```rust
        assert_eq!(
            hkdf_sha256(&udp_ikm, &SESSION_SALT, b"maho/udp-c2h/v3/nonce", 4),
            [0xe2, 0x6c, 0x23, 0xc1]
        );
```
```rust
    #[test]
    fn bootstrap_key_is_deterministic_per_pin() {
        let expected = [
            0x2f, 0x88, 0x83, 0xb2, 0xf5, 0x6f, 0x8d, 0xe0, 0x2a, 0xc8, 0x8b, 0x3b, 0xf9, 0x21,
            0x3b, 0x70, 0x1d, 0xde, 0x0e, 0x35, 0x31, 0xc0, 0x4c, 0x9f, 0x29, 0xc5, 0xf0, 0x5f,
            0xd7, 0x4a, 0xe6, 0xfb,
        ];
        assert_eq!(bootstrap_psk("12345678").unwrap(), expected);
```
```rust
pub(crate) fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let hkdf = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut output = vec![0_u8; length];
    hkdf.expand(info, &mut output)
        .expect("all protocol HKDF output lengths are valid");
    output
}
```
- **Impact**: These assertions cannot agree with the shown standard HKDF implementation and their test inputs, so the golden-vector tests are not a usable passing compatibility check. Independent Python HMAC-SHA256/HKDF calculations, first validated against both RFC 5869 cases in `udp_gcm.rs`, produce topic `erd3-5cb37a6711bb98f44cf615c77173` for `12345678`, client-to-host nonce prefix `329bb912` for the UDP fixture, and bootstrap key `40b939d47498c89fb2bafe148b74333d35c084225d8f045c7505044276f6243a` after the configured 600,000 PBKDF2 rounds. All differ from the quoted expected values. Rust tests were not executed in this read-only lane; these are independently calculated deterministic contradictions, not claimed cargo-test output or a compile failure. Whether another implementation follows these fixtures is outside the reviewed scope.
- **Fix**: Reconcile the protocol's authoritative derivation parameters with an independent implementation, then correct the inconsistent expected topic, key, nonce, bootstrap, and dependent full-datagram vectors together. If the current parameters are authoritative, use the independently calculated outputs above. Do not alter a standard HKDF implementation simply to satisfy unrelated fixture bytes; run the existing vector assertions against the reconciled protocol.
- **Confidence**: high

## Non-findings checked

- UDP uses distinct `maho/udp-c2h/v3` and `maho/udp-h2c/v3` key labels; even a four-byte prefix collision would not mean the two directions share a key.
- UDP nonce construction is four derived prefix bytes plus an eight-byte big-endian counter; increment is checked before encryption and cannot wrap.
- Failed encryption attempts consume their counter; exhaustion leaves the caller's output buffer and terminal counter unchanged.
- Per-cipher nonce uniqueness is preserved; recreating a cipher with the same master key, salt, and direction recreates its stream, so cross-session salt freshness remains an explicit unverified caller obligation.
- The replay window accepts unseen counters at distance 4095 and rejects distance 4096; the lower-bound calculation and block pruning agree, including at zero and large counter jumps.
- Replay state is committed only after successful GCM authentication, so forged high counters cannot evict authenticated packets from the window.
- Replay bitmap retention is bounded to at most 65 blocks for a 4096-counter window; stale counters cannot recreate pruned blocks through `open`.
- Packet headers are authenticated as AAD; the fixed-size header and nonce are checked before indexing, and a decoded header is not returned unless authentication succeeds.
- HKDF key, nonce, UDP-IKM, signaling-topic, signaling-payload, and TLS-bootstrap labels are separated; the signaling defect is weak input entropy/guessing cost, not a label collision.
- Signaling encryption obtains a new 96-bit nonce from OpenSSL's CSPRNG for each encryption; no deterministic nonce reset was found there.
- Pairing identities are constructed with the exact `maho-p1.` prefix, bootstrap uses exactly `maho-b1`, embedded NULs are rejected, and the TLS server matches the complete offered identity rather than a prefix.
- Non-constant-time comparisons in these files concern public PSK identities, transmitted nonce prefixes, or transmitted STUN transaction IDs, not secret keys or MAC tags; GCM/TLS authentication is delegated to their crypto libraries.
- Bootstrap PIN validation requires exactly eight ASCII digits, and TLS bootstrap derivation applies PBKDF2-HMAC-SHA256 before HKDF.
- `BootstrapLockout` cancels pairing on the fifth recorded failure in its anchored attempt window; timeout expiry does not silently reactivate pairing, while explicit `begin_pairing` resets the state. Rolling-window semantics and host call-site enforcement are not claimed.
- The TLS deadline helpers use nonblocking I/O and repeatedly check the supplied absolute deadline around readiness and handshake/read retries; partial TLS records do not create a new budget inside these helpers.
- TLS ciphers are restricted to PSK AES-GCM, TLS 1.3 is disabled for the legacy callback path, and the permissive certificate callback does not introduce a certificate-authenticated fallback cipher.
- TCP receive framing rejects zero and greater-than-16-MiB lengths; transport reads are bounded and pending events are drained before another read, so coalesced tiny frames do not create an indefinitely growing event queue.
- TCP outbound admission is bounded by both frame count and retained bytes; queue rejection precedes encoding/allocation, and terminal write errors forbid subsequent ciphertext.
- STUN uses a fixed 2048-byte receive buffer, validates type/cookie/transaction ID, checks advertised attribute bounds before slicing, and advances the attribute loop even for zero-length values.
- Reviewed production `expect`/`unwrap` sites in these modules have fixed-length or local-state invariants; no peer-controlled panic from those sites was established.

Verification: only the requested findings file was written. The independent Python derivation check passed RFC 5869 cases 1 and 3 and the offline-PIN demonstration; its initial comparison against the repository's signaling golden value failed, which led to the separately reported vector discrepancy. Source evidence was reopened at the cited lines, and the completed report's quotations and line count were checked mechanically. No Rust test execution or full admission-path validation is claimed.
