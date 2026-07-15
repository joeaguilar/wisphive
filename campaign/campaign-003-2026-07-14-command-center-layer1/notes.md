# campaign-003 — Command Center Layer 1

- G: close epic #398 (all four children live w/ runtime evidence). #399 shipped Sprint-2; this campaign delivers #400/#401/#402 + fixes inbox regression #473.
- Model: PO switched session to Fable 5 at approval — workers inherit it (matches route:fable-5 tags; #473 security work gets top-tier model).
- Wave shape: W1 = #473 (hook, isolated) ‖ #400 (liveness board). W2 = #401. W3 = #402. W2/W3 serialized on shared frontend shell (useWisphive.ts, types/protocol.ts, wire.rs, nav).
- Hard rules baked into worker prompts:
  - Spec §5: state mirror, NOT steering wheel. No write affordances (#401 explicitly).
  - Runtime evidence mandatory (isolated daemon, SHORT HOME for SUN_LEN, scratch @playwright/test script — MCP browser can't ignore self-signed TLS).
  - e2e churns sprint evidence PNGs — never stage them.
  - "User sees X" ⇒ test full untruncated X reachable.
  - Conventional Commits, stage own files only, commit on current branch crossfire-blitz/20260712-230112.
  - #473: must not regress itr#388 (PermissionRequest still returns Ask); verify against INSTALLED binary; reinstall via ./install.sh.
- Pre-existing dirty files (NOT ours, never stage): sprint-2 evidence PNGs, docs/decisions/0009*, docs/decisions/README.md.
- Roadmap correction queued in packet: Phase 2 (#403) done in itr, unmarked in ROADMAP.
