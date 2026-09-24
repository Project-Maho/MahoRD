# Remote connection and pairing implementation

Source plan: `docs/remote-connection-pairing-review-plan-20260910.md`.

Append-only execution notepad:
`/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH`

Tier: HEAVY. Implementation: Gemini 3.8 Flash. Final review: this session's lead.

Phase-scoped workflow runs: A trust, B UDP registration, C identity/discovery, D mobile lifecycle, E integration. Final coordinator-owned node: `lead-final-review`.

Evidence belongs under `evidence/`; node reports under `reports/`. No production credentials in these files. All Rust compilation and tests run on Omarchy except specifically physical iOS builds/signing on Mac. No permanent deployment or public release.
