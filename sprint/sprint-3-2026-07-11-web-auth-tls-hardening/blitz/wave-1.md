# Blitz run — Sprint-3 (2026-07-11)

## Config

- Tracker: `itr` (list: `itr get 496` + member IDs; close: `itr close <id>`)
- Epic: itr#496 — Sprint-3: Web-auth / TLS hardening
- Dep graph: `kgr` present. `kgr check` at preflight showed pre-existing cycles (state/mod.rs, daemon lib.rs/server.rs, web lib.rs/http_tests.rs) and 6 orphan scripts — none touch this backlog's files, not addressed by this blitz.
- Verify gate (per-agent): `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all --check`
  - Frontend stories (#275, #280, #492) additionally: `cd crates/wisphive_web/frontend && npm run lint && npm test`
  - #492 additionally: `npm audit && npm audit --omit=dev` (its own AC)
  - Rust stories additionally: `cargo deny check advisories bans sources` (sprint DoD "security dep gate")
- Verify gate (wave gate, orchestrator-run between waves): `just verify` (full suite incl. e2e, per CLAUDE.md)
- Concurrency: 5 max (plan uses ≤4/wave — file-contention bound, not concurrency bound)
- Repos: `.` (wisphive monorepo)
- Stop when: backlog empty | 2 consecutive no-progress waves | foundational quarantine blocks | max_waves reached (none set)

## Waves

Driven by two 7-task file-conflict chains (`auth.rs`, `tls.rs`) that don't overlap each other, so they run as parallel lanes across 7 waves. `security.rs` (4 tasks) and `lib.rs` (5 tasks, one — #256 — shared with security.rs) interleave into whichever waves have room, ordered risk-first (high risk earliest).

| Wave | Task | File(s) | Risk |
|---|---|---|---|
| 1 | itr#317 | lib.rs, security.rs, auth.rs | high |
| 1 | itr#228 | tls.rs | high |
| 1 | itr#275 | frontend/src/hooks/useWisphive.ts | high |
| 1 | itr#492 | frontend/package.json, package-lock.json | med |
| 2 | itr#237 | tls.rs | high |
| 2 | itr#232 | auth.rs | med |
| 2 | itr#494 | security.rs, http_tests.rs | high |
| 2 | itr#258 | lib.rs, wisphive_daemon/src/state.rs | low |
| 3 | itr#226 | tls.rs | med |
| 3 | itr#233 | auth.rs | med |
| 3 | itr#245 | security.rs | high |
| 3 | itr#259 | lib.rs | low |
| 4 | itr#227 | tls.rs | med |
| 4 | itr#243 | auth.rs | med |
| 4 | itr#256 | security.rs, lib.rs | high |
| 5 | itr#235 | tls.rs | med |
| 5 | itr#246 | auth.rs | med |
| 5 | itr#280 | lib.rs, frontend/src/components/Login.tsx | low |
| 6 | itr#236 | tls.rs | med |
| 6 | itr#244 | auth.rs | low |
| 7 | itr#234 | tls.rs | low |
| 7 | itr#247 | auth.rs | low |

## File conflicts

- `security.rs`: #494, #245, #256, #317 — serialized across waves 1, 2, 3, 4.
- `lib.rs`: #256, #317, #258, #259, #280 — serialized across waves 1, 2, 3, 4, 5.
- `auth.rs`: #317, #232, #233, #243, #246, #244, #247 — serialized across all 7 waves.
- `tls.rs`: #228, #237, #226, #227, #235, #236, #234 — serialized across all 7 waves.
- No two same-wave tasks share a file (verified against each story's `FILES:` itr field).

## Semantic warnings

- Wave1→2: #317 (auth.rs, adds `ParsedOrigin` extension + peek-throttle) lands before #232 (auth.rs, Argon2 param validation on `verify_password`) — sequential by wave, no action needed.
- Wave2→3: #232 (auth.rs, tightens `verify_password`) lands before #245 (security.rs, wraps `verify_password` call site in a timeout) — #245's agent should treat #232's validation as already-landed behavior of `verify_password`, not something to re-derive.
- tls.rs cluster (#228, #237, #226, #227, #235, #236, #234) all touch cert/key lifecycle (`ensure_cert`, `try_load_existing`, `FileLock`) in overlapping conceptual territory but different functions/tests — sequenced one-per-wave, self-healing gate is the safety net for any accidental overlap.
- #494 and #317 both touch request-auth-gate plumbing in `security.rs`/`lib.rs` (query-token rejection vs. ParsedOrigin/peek-throttle) — different call sites, no direct overlap, but both land in the same subsystem; wave-gate `just verify` after wave 2 is the checkpoint.
- Runtime evidence: #280 needs a driven UI check (12-char password message is user-visible); #275 and #494 satisfy "runtime/HTTP-test proof" via their own required automated tests (Vitest approve-stash test; `http_tests.rs` query-token test) since neither has a visual surface; #492 is dependency-only, exempt from Playwright.

## Interventions

- **Wave 1 / itr#275**: agent finished its own work (tool_name cross-check in `useWisphive.ts` + 3 new Vitest tests, all passing) but held off closing because `cargo fmt --all --check` was red — solely inside itr#317's in-flight files (`lib.rs`, `security.rs`), which itr#275 correctly did not touch. Once itr#317 landed and closed (full green gate), orchestrator re-ran the full gate (`cargo test --workspace`, `cargo clippy -- -D warnings`, `cargo fmt --all --check`, `cargo deny check advisories bans sources`) — all green — and closed itr#275 directly (`itr close 275`). No re-spawn needed; this was an inspect-and-close, not new work.
- **Wave 1 / itr#228**: agent self-reported two transient red states during its own verify loop, caused by itr#317 actively mid-edit on `security.rs`/`lib.rs` (compile error → unused-import warnings → fmt drift, in that order). Agent correctly did not touch those files, polled until they settled, then confirmed its own full chained gate green and closed itself (`itr close 228`). No orchestrator action needed — self-resolved.

## Outcomes

### Wave 1 (closed)

| Task | Result | Notes |
|---|---|---|
| itr#317 | closed | `PeekThrottle` (auth.rs) + `ParsedOrigin` extension (security.rs/lib.rs); 9 new tests; gate green including `cargo deny` |
| itr#228 | closed | SPKI mismatch check in `try_load_existing` (tls.rs); promoted `x509_parser` to a real dep in `wisphive_web/Cargo.toml`; 2 new tests |
| itr#275 | closed (orchestrator intervention) | tool_name cross-check on approve-stash replay (useWisphive.ts); 3 new Vitest tests; closed after itr#317 settled |
| itr#492 | closed | `npm audit fix` — all 4 dev-dep vulns resolved via minor lockfile bumps only (Vite 8.0.13→8.1.4); no major-version risk materialized |

Files touched in Wave 1: `crates/wisphive_web/src/auth.rs`, `crates/wisphive_web/src/security.rs`, `crates/wisphive_web/src/lib.rs`, `crates/wisphive_web/src/tls.rs`, `crates/wisphive_web/Cargo.toml`, `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/hooks/useWisphive.test.ts`, `crates/wisphive_web/frontend/package-lock.json`.

**Wave 1 gate**: orchestrator ran full `just verify` (fmt, clippy, cargo test, frontend lint+vitest, e2e) after all 4 tasks closed — all green, including 11/11 Playwright e2e specs.

### Wave 2 (closed)

| Task | Result | Notes |
|---|---|---|
| itr#237 | closed | Reused itr#228's `key_matches_cert_spki` helper to strengthen `concurrent_ensure_cert_serializes`; test-only change |
| itr#232 | closed | Argon2 algorithm/param downgrade check in `verify_password`; 4 new tests including exact-minimum boundary case |
| itr#494 | closed | Scoped query-string token auth to `/ws` only via `path_query_token_allowed`; flipped `api_config_accepts_valid_device_token_via_query` → `_rejects_...`; also fixed a break in `http_tests.rs` caused by itr#258's JSON-audit-format change (same-file, in-scope fix) |
| itr#258 | closed (orchestrator intervention) | JSON-formatted `append_web_audit` detail via new `audit_reason()` helper in lib.rs; itr's `FILES:` field pointed at a nonexistent `state.rs` — actual file is `crates/wisphive_daemon/src/state/web_auth.rs` (test added in sibling `state/web_auth_tests.rs`); held off closing because its change broke one assertion in `http_tests.rs` (owned by itr#494 this wave) — itr#494's agent fixed that assertion as part of its own in-scope work, orchestrator re-ran the full gate (green) and closed itr#258 directly |

## Interventions (continued)

- **Wave 2 / itr#258**: itr's `FILES:` field named a nonexistent `crates/wisphive_daemon/src/state.rs`; agent prompt corrected this to `crates/wisphive_daemon/src/state/web_auth.rs` before spawning (confirmed via `grep -rl "fn append_web_audit"`). No orchestrator action needed post-hoc — caught at Phase 2 wave-plan time.
- **Wave 2 / itr#258 close**: cross-task test conflict — itr#258's JSON-audit-format change broke an assertion in `http_tests.rs` (owned by itr#494). itr#494's own agent fixed the assertion as legitimate in-scope work on its owned file, then closed itself. Orchestrator re-ran the full gate (green) and closed itr#258 directly (`itr close 258`) — inspect-and-close, no re-spawn.

Files touched in Wave 2: `crates/wisphive_web/src/tls.rs`, `crates/wisphive_web/src/auth.rs`, `crates/wisphive_web/src/security.rs`, `crates/wisphive_web/src/http_tests.rs`, `crates/wisphive_web/src/lib.rs`, `crates/wisphive_daemon/src/state/web_auth.rs`, `crates/wisphive_daemon/src/state/web_auth_tests.rs`.

**Wave 2 gate**: full `just verify` green (fmt, clippy, cargo test, frontend lint+vitest, e2e 11/11).

### Wave 3 (closed)

| Task | Result | Notes |
|---|---|---|
| itr#226 | closed | DER `NotBefore` cross-check via `der_not_before_unix` (reuses itr#228's x509_parser pattern); regen even if sidecar `created_at` lies |
| itr#233 | closed | `apply_failure` only advances `locked_until` from `max(now, locked_until)`; 100-failures-during-lockout test |
| itr#245 | closed (orchestrator intervention) | Investigation-only: acceptance already satisfied by itr#213 (`VERIFY_DEADLINE` timeout wrapping `verify_password` in `lib.rs`, landed prior to this sprint); itr#245 was simply never closed. No code change made. Held for gate green (blocked transiently by itr#233's uncommitted fmt drift in auth.rs), orchestrator re-verified green post-itr#233 and closed directly |
| itr#259 | closed | `/api/devices` redacts `last_ip` to `None` for non-caller devices; new `devices_hides_last_ip_for_other_devices` HTTP integration test |

## Interventions (continued)

- **Wave 3 / itr#245**: agent determined no code change was needed (fix already shipped under itr#213) but withheld closing solely because of transient fmt drift in itr#233's (neighbor, same wave) uncommitted `auth.rs` work. Once itr#233 closed (its own agent hand-fixed its fmt drift), orchestrator re-ran the full gate (green) and closed itr#245 directly (`itr close 245`) — inspect-and-close, no code changed by anyone for this ticket.
- Noted, not actioned: itr#259's agent saw one flaky, pre-existing, unrelated `wisphive_hook::tests::socket_garbage_decision_fails_closed` failure on an isolated run; passed on retry and in the final full gate. Not caused by this blitz; no follow-up filed (single non-reproducing flake).

Files touched in Wave 3: `crates/wisphive_web/src/tls.rs`, `crates/wisphive_web/src/auth.rs`, `crates/wisphive_web/src/lib.rs`, `crates/wisphive_web/src/http_tests.rs`. (itr#245 touched no files.)

**Wave 3 gate**: full `just verify` green (fmt, clippy, cargo test, frontend lint+vitest, e2e 11/11 — `/api/devices` redaction didn't break the existing devices e2e spec).

### Wave 4 (closed)

| Task | Result | Notes |
|---|---|---|
| itr#227 | closed | SAN/LAN-URL filtering (`is_virtual_iface_name`, `is_rfc1918`, `usable_lan_ipv4_addrs`); synthetic-interface tests avoid depending on real Docker in CI. Unblocked itr#270 per agent's own note. |
| itr#243 | closed (orchestrator intervention) | `WISPHIVE_MAX_IN_FLIGHT_PER_IP` env var configures `LoginThrottle`'s in-flight cap, default stays 1; held for gate green (blocked by itr#256's in-flight fmt drift), orchestrator re-verified green post-itr#256 and closed directly |
| itr#256 | closed | `password_set_cache: AtomicBool` on `SecurityConfigInner`, cache-aside `password_set()` + `mark_password_set()`; documented as a deliberate one-way "sticky true" latch (no cross-process invalidation path exists for CLI `reset-password`, and a reset also revokes all tokens regardless) rather than leaving it unaddressed |

## Interventions (continued)

- **Wave 4 / itr#227 — process deviation**: this agent's own gate reported `cargo fmt --all --check` red (drift in itr#256's in-flight `security.rs`, neighbor-owned) but it ran `itr close 227` anyway, unlike itr#237/#245/#243 in earlier waves which correctly held off. The underlying `tls.rs` work was independently verified sound (its own tests/clippy/deny all green, drift was 100% in a file it didn't touch) and the wave-gate re-run after itr#256 landed confirmed everything green — no harm resulted, but flagging the deviation from the "stop and report instead of closing" instruction for awareness.
- **Wave 4 / itr#243 close**: held correctly (same root cause as above — itr#256's in-flight fmt drift). itr#256's own agent hand-fixed that exact drift as part of its own work. Orchestrator re-ran the full gate (green) and closed itr#243 directly (`itr close 243`) — inspect-and-close, no re-spawn.

Files touched in Wave 4: `crates/wisphive_web/src/tls.rs`, `crates/wisphive_web/src/auth.rs`, `crates/wisphive_web/src/security.rs`, `crates/wisphive_web/src/lib.rs`.

**Wave 4 gate**: full `just verify` green (fmt, clippy, cargo test, frontend lint+vitest, e2e 11/11).

### Wave 5 (closed)

| Task | Result | Notes |
|---|---|---|
| itr#235 | closed (orchestrator intervention) | Randomized tmp-filename suffix (`rand::rngs::OsRng`, 8 bytes hex) for cert/key writes; removed the pre-write `remove_file` TOCTOU helper. Held for gate green (blocked by itr#246's in-flight auth.rs), orchestrator re-verified green post-itr#246 and closed directly |
| itr#246 | closed | `backoff_for` cap now configurable via `WISPHIVE_BACKOFF_CAP_SECS`, default unchanged at 30s — deliberately did NOT hardcode the ticket's suggested 5min per sprint plan's own Non-Goal ("no throttle calibration from real UX telemetry") |
| itr#280 | closed | `MIN_PASSWORD_LEN` 8→12 in lib.rs + Login.tsx; **real Playwright runtime evidence captured** (temp spec, deleted after use) — 11-char blocked via native `minLength`, 12-char accepted through to dashboard; screenshots in session scratchpad |

## Interventions (continued)

- **Wave 5 / itr#235 close**: held correctly on itr#246's in-flight `auth.rs` (backoff_cap threading mid-edit, confirmed via 3 consecutive compile-error checks isolated to that file). Orchestrator re-ran the full gate after itr#246 closed (green) and closed itr#235 directly (`itr close 235`) — inspect-and-close, no re-spawn.

## Follow-up candidates (not filed yet — surfaced during Wave 5, out of scope for this sprint)

- itr#280's agent found that `Login.tsx`'s native HTML `minLength` attribute intercepts form submission before the custom JS `localError` message ("Password must be at least 12 characters.") can render — the custom copy is effectively unreachable via a real mouse/keyboard submit. Pre-existing behavior (same pattern existed at the old MIN_PASSWORD_LEN=8), not introduced or worsened by itr#280. Worth a low-priority follow-up ticket if the team wants the custom message to actually surface (e.g. for browsers suppressing native validation UI). Flagging for `/sprint-review` triage rather than filing mid-blitz.

Files touched in Wave 5: `crates/wisphive_web/src/tls.rs`, `crates/wisphive_web/src/auth.rs`, `crates/wisphive_web/src/lib.rs`, `crates/wisphive_web/frontend/src/components/Login.tsx`.

**Wave 5 gate**: full `just verify` green (fmt, clippy, cargo test, frontend lint+vitest, e2e 11/11).

### Wave 6 (closed)

| Task | Result | Notes |
|---|---|---|
| itr#236 | closed | Genuine cross-process `flock` test: 5 real child processes via `Command::new(current_exe())` re-invoked as a helper, filesystem barrier for simultaneous release, converge to one fingerprint/one regen; verified non-flaky over 3 repeated runs |
| itr#244 | closed | `parallel_attempts_one_in_flight_held_across_sleep`: winner holds guard across a real `tokio::time::sleep` under `start_paused`, 9 racers all rejected — belt-and-suspenders alongside the existing yield-based test |

Files touched in Wave 6: `crates/wisphive_web/src/tls.rs`, `crates/wisphive_web/src/auth.rs`.

**Wave 6 gate**: full `just verify` green (fmt, clippy, cargo test, frontend lint+vitest, e2e 11/11).

### Wave 7 (closed) — final wave

| Task | Result | Notes |
|---|---|---|
| itr#234 | closed | In-process `static Mutex<()>` taken before `flock` in `FileLock::acquire_exclusive`; new `acquire_exclusive_on` test entry point exercises a real `dup(2)`'d shared fd across 8 threads; **agent proved the test isn't vacuous** by temporarily disabling the mutex, confirming the test fails (`max_concurrent == 8`) without the fix, then restored and re-verified |
| itr#247 | closed | `sha256_hex` rewritten via `std::fmt::Write` into one pre-sized `String` — 32 allocations → 1, byte-identical output confirmed by existing `token_verify_roundtrip`/`argon2_roundtrip` tests |

Files touched in Wave 7: `crates/wisphive_web/src/tls.rs`, `crates/wisphive_web/src/auth.rs`.

**Wave 7 gate**: full `just verify` green (fmt, clippy, cargo test, frontend lint+vitest, e2e 11/11). **Backlog empty — all 22 stories closed. Blitz complete.**

## Final summary

All 22 sprint-3 stories closed across 7 waves, zero quarantines, zero foundational blocks. `just verify` green after every wave gate (7/7 checkpoints), including 11/11 Playwright e2e specs each time.

**Diff stat**: 11 files changed in `crates/wisphive_web` + `crates/wisphive_daemon` + frontend, 2736 insertions / 393 deletions (excludes the pre-existing sprint-2 evidence PNG churn from `just e2e` runs, which predates this blitz and was not staged).

**Orchestrator interventions**: 6 inspect-and-close (itr#275, #258, #245, #243, #235 — all cases of an agent correctly holding a green-on-its-own-file task until a same-wave neighbor's in-flight file settled) + 1 process-deviation note (itr#227 closed despite a neighbor's transient red gate; harmless in outcome, re-verified green at the wave boundary). No re-spawns were needed — every hold was resolved by the neighbor's own agent landing, confirming the file-ownership model self-heals as designed.

**Follow-up candidate for /sprint-review triage**: Login.tsx's native `minLength` HTML attribute intercepts form submission before the custom JS password-length error message can render (pre-existing, not introduced by itr#280) — low-priority UX polish, not filed as an issue yet.

**Epic itr#496** left open per project convention — closing epics is a `/sprint-review` responsibility, not `/blitz`'s.

**Next steps**: run `/sprint-review` to walk acceptance, fill Outcomes/Demo/Retro in `plan.md`, triage the Login.tsx follow-up, and close itr#496. Review the diff and commit — nothing was committed during this blitz.

## Post-blitz addendum (2026-07-11, same day)

The 7-wave run above was committed as `a38ea02`, then run through `/crossfire-review` (Codex adversarial-review + Opus, independent lanes). Both lanes converged on itr#497 (sticky `password_set` cache breaking live `reset-password`) — strong corroboration. 7 findings total filed (itr#497–503), all parented to itr#496, groomed via `/groom`, and added to `sprint-3/plan.md` as **Wave 8** (see plan.md for the full table, complexity/route tags, and file-contention notes: `auth.rs`×3, `tls.rs`×3, `security.rs` standalone). Wave 8 has not been executed yet — this file will gain new wave sections when it runs, following the same file-ownership/verify-gate/wave-gate pattern as Waves 1–7. itr#503 needs a maintainer decision (self-heal vs. hard-fail semantics) before it's dispatchable; the rest are ready.
