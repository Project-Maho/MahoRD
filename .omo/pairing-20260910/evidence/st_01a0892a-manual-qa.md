# Manual QA Matrix: st_01a0892a

## Surface Evidence

| scenarioId | criterionRef | surface | exactInvocation | verdict | artifactRefs |
|---|---|---|---|---|---|
| QA-01 | CRIT-R2-BOOTSTRAP-AUTH-SUITE | CLI (Remote Omarchy Linux) | `ssh -o BatchMode=yes indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib cargo test --manifest-path clients/rust/Cargo.toml -p erd-host bootstrap"` | PASS | art-bootstrap-log, art-report |
| QA-02 | CRIT-R11-PIN-SELECTION-SUITE | CLI (Remote Omarchy Linux) | `ssh -o BatchMode=yes indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib cargo test --manifest-path clients/rust/Cargo.toml -p erd-host pin"` | PASS | art-pin-log, art-report |
| QA-03 | CRIT-HOST-PACKAGE-REGRESSION | CLI (Remote Omarchy Linux) | `ssh -o BatchMode=yes indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib cargo test --manifest-path clients/rust/Cargo.toml -p erd-host"` | PASS | art-bootstrap-log, art-pin-log, art-report |
| QA-04 | CRIT-CLIPPY-CLEAN | CLI (Remote Omarchy Linux) | `ssh -o BatchMode=yes indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-host --all-targets --no-deps -- -D warnings"` | PASS | art-clippy-log, art-report |
| QA-05 | CRIT-RED-REGRESSION-CAPTURED | CLI (Remote Omarchy Linux) | `ssh -o BatchMode=yes indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib cargo test --manifest-path clients/rust/Cargo.toml -p erd-host pin && cargo test --manifest-path clients/rust/Cargo.toml -p erd-host bootstrap"` | PASS | art-red-log, art-report |

## Adversarial Cases

| scenarioId | criterionRef | adversarialClass | expectedBehavior | verdict | artifactRefs |
|---|---|---|---|---|---|
| ADV-01 | CRIT-R2-UNCONSENTED-HANDSHAKE | Authentication Bypass | Connecting via bootstrap TLS (`erd-b1`) and sending `Handshake(A)` without operator consent must terminate connection with `SessionError::PreAuth` and never start media capture or input injection | PASS | art-bootstrap-log, art-report |
| ADV-02 | CRIT-R2-CONSENT-ID-CONFUSION | Identity Substitution | Connecting via bootstrap TLS, obtaining consent for client B, but transmitting `Handshake(A)` must be rejected with `SessionError::IdentityMismatch` without starting media capture | PASS | art-bootstrap-log, art-report |
| ADV-03 | CRIT-R2-DUPLICATE-HANDSHAKE | State Machine Mutation | Transmitting a second `Handshake` packet on an already authenticated session must be rejected with `SessionError::AlreadyAuthenticated`, leaving media capture and ciphers unmutated | PASS | art-bootstrap-log, art-report |
| ADV-04 | CRIT-R11-PROBABILISTIC-AVOIDANCE | Flaky Test Avoidance | Default PIN verification must test execution of injected generator branch rather than comparing random sample to fixed constant | PASS | art-pin-log, art-report |
| ADV-05 | CRIT-SCOPE-NON-INTERFERENCE | Concurrency / Task Ownership | Only assigned files `session.rs` and `main.rs` modified; upstream R1 store changes preserved; zero edits to `erd-app` or shells | PASS | art-report |

## Artifact References

| id | kind | description | path |
|---|---|---|---|
| art-report | Markdown Report | Phase A consent and PIN implementation report | `.omo/pairing-20260910/reports/a-consent.md` |
| art-bootstrap-log | Test Log | Execution log for `cargo test -p erd-host bootstrap` passing all 7 tests on Omarchy remote host | `.omo/pairing-20260910/evidence/st_01a0892a-bootstrap-tests.log` |
| art-pin-log | Test Log | Execution log for `cargo test -p erd-host pin` passing all 5 tests on Omarchy remote host | `.omo/pairing-20260910/evidence/st_01a0892a-pin-tests.log` |
| art-clippy-log | Clippy Log | Zero warnings under `-D warnings` for `erd-host` on Omarchy remote host | `.omo/pairing-20260910/evidence/st_01a0892a-clippy.log` |
| art-red-log | Regression Log | Output of failing tests before production changes confirming R2 and R11 regressions | `.omo/pairing-20260910/evidence/st_01a0892a-red-regression.log` |
