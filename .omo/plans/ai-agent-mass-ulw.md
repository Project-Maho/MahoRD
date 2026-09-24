# Plan: AI Agent Remote Desktop Full Implementation & Code Review

## Phase 1: Security & Input Safety Baseline (P0)
- Task 1.1: `erd-app`: Implement Local HTTP Control Plane Authentication (Bearer token, `X-ERD-Token`, Origin checks, remove `*` CORS) and token generation/CLI flag in `erd-client`.
- Task 1.2: `erd-app`: Fix Input Safety Tracker & Watchdog (activate periodic check_timeout watchdog loop, ensure held keys/buttons release on disconnect before clear, macOS host Reset support).
- Task 1.3: `erd-proto` & `erd-host`: Implement Input ACK Protocol and event completeness (Right drag event, modifier tracking, wire ACK packet).

## Phase 2: Coordinates, High-DPI & Unicode Injection (P0/P1)
- Task 2.1: `erd-app` & `erd-proto`: Coordinate Contract & High-DPI Unification (screen_info and screenshot metadata reporting physical/logical dimensions and scale factor, normalize/pixel space translation).
- Task 2.2: `erd-host` & `erd-app`: Unicode Text Injection Engine (Windows SendInput KEYEVENTF_UNICODE, Linux uinput keysym/unicode, proper typing handling).
- Task 2.3: `erd-app` & `erd-host`: Multi-Monitor Enumeration & Coordinate Mapping (expose monitor geometry in screen_info, per-monitor input mapping).

## Phase 3: Screen Freshness & Agent Perception Feedback (P1)
- Task 3.1: `erd-app` & `erd-decode`: Screen Freshness Metadata (attach frame_id, capture timestamp, and age to screenshot/screen_info).
- Task 3.2: `erd-app`: Wait-For-Screen-Change / Screen Diff API (add `/api/v1/screen/wait_change` and MCP tool).

## Phase 4: Omarchy Linux Build, Test & Code Review
- Task 4.1: Rsync to Omarchy (`indo@100.91.254.71`) and run workspace `cargo check` and `cargo test`.
- Task 4.2: Real-surface verification: Exercise agent HTTP API and MCP with authentication and test actions.
- Task 4.3: Write comprehensive Code Review and Verification Report in `docs/ai-agent-implementation-review.md`.
