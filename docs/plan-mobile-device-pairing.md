# Plan: Mobile Device Pairing (Unlock Workflow)

## Goal

Let a logged-in operator pair a phone with Wisphive in under a minute. Phone runs the same web UI, authenticates as a first-class device, and approves/denies decisions from the same queue the desktop TUI sees. Zero CLI required on either side.

This doc is the scope-and-sequencing ground truth for the mobile pairing workstream. It supersedes any ad-hoc sizing in the individual itr issues — those are correct at the contract level but understate the hidden design work that has to land somewhere.

## What's already done (foundations)

The plumbing is further along than it looks from the issue list. The work below is already merged on `main` and changes how we should size the remaining items:

- **TLS cert management** (`crates/wisphive_web/src/tls.rs`): self-signed ECDSA P-256 with 397-day cap + 24h clock-skew backdate, atomic rotation under `flock`, SAN sidecar for drift detection, fingerprint CLI. `ensure_cert` is race-safe across concurrent daemon starts.
- **Per-device bearer tokens** with Argon2id password + SHA-256 token-hash storage, per-IP login throttle with fail-closed `AttemptGuard`, h2 `:authority` handling, CSRF origin/host allowlist (`crates/wisphive_web/src/security.rs`).
- **Set-password web endpoint** (`POST /api/auth/set-password`, itr#268) — landed in `5f7d0fa` + `984740f`, ready to close. Atomic `try_set_initial_web_password`, body-size caps, failure-path audit rows, 409 on already-set.
- **First-run browser auto-open** (itr#267) — `daemon start --web` / `web serve` open the SPA on a fresh install; `--no-open` suppresses for CI.
- **WebAuthn schema** already wired in `wisphive_daemon::state`: `web_passkeys` table, cascade-delete on device revoke, `insert_web_passkey` / `list_web_passkeys_for_device`, reset-password cascade. `webauthn-rs = "0.5"` is a workspace dep and linked into `wisphive_web`. What's missing is the HTTP surface and frontend glue — not the DB work.

Net: the crypto, persistence, and gate machinery exist. The remaining work is (a) stabilizing the SAN set, (b) extending the TLS/allowlist surfaces to LAN IPs, (c) building the pairing-token state machine, (d) writing the WebAuthn handlers, and (e) the two frontends.

## Critical path

```
          itr#268 ──► itr#269            (Stage 1: set password via web UI)
              │           │
              │           └──────────┐
              │                      │
itr#227 ──► itr#270 ──► itr#271 ──► itr#272
  (SAN      (LAN IP     (pair         (desktop QR +
  filter)   in SAN +    token +       mobile /pair +
            user cert)  ephemeral     WebAuthn glue)
                        listener)
              │
              └──► itr#219           (WebAuthn register/login handlers)
                      ▲
                      └── parallel to #270/#271, must land before #272 ships
```

Strict ordering: `#227 → #270 → #271 → #272`. `#219` runs parallel to `#270`/`#271` and merges into `#272`. `#268` is done; `#269` can land any time after `#268` closes.

## Per-item deep dive

Each section is "what the issue says" → "what the code actually requires" → "risks and estimate."

### itr#227 — TLS SAN filter (blocks #270)

**Surface:** one function in `crates/wisphive_web/src/tls.rs`: `compute_sans` (and the parallel `enumerate_lan_urls`). Filter `if_addrs::get_if_addrs()` output to reject `docker*`, `br-*`, `utun*`, `tun*`, `tap*`, `vnic*`, `vbox*` interface names and CGNAT `100.64.0.0/10` (unless opted in for Tailscale users).

**Real work:** adding the filter is ~40 LOC. The wrinkle is that `if_addrs::Interface` only gives us `name`, `ip()`, and `is_loopback()` — not a routing-table default-interface signal. We can filter by name+range, but we can't pick "the" LAN IP; we can only narrow the set. For #270 that's enough. For the trusted-cert path it's moot.

**Testability:** hermetic unit test is hard (we can't easily fabricate `if_addrs` output). Acceptance needs a Docker-running laptop. Pragmatic fix: refactor `compute_sans` to take an `impl IntoIterator<Item=IfInfo>` so tests can inject.

**Estimate:** 0.5 day including the injection refactor + name/range tests.

**Hidden risk:** the filter list is easy to under-specify. macOS 15 adds new vendor interfaces (e.g. `awdl0`, `llw0`) that we'll want to skip. Safer rule: allow-list RFC1918 (`10/8`, `172.16/12`, `192.168/16`) and reject everything else, with an env-var escape hatch.

### itr#270 — TLS cert with LAN IP in SAN + user-provided cert (blocks #271, #272)

**Surface:** `tls.rs` (extend SANs), `lib.rs::serve` (thread a new cert-source option), `main.rs` (two new CLI flags: `--tls-cert`, `--tls-key` on `daemon start` and `web serve`).

**Real work breakdown:**
1. SAN now includes the "primary LAN IP" — requires choosing one when multiple non-filtered interfaces exist. Rule: if `bind_host == 0.0.0.0` pick the first RFC1918 v4 that survived #227's filter; if `bind_host` is explicit, use that. Log a warning on ambiguity.
2. New `enum CertSource { SelfSigned, UserProvided { cert: PathBuf, key: PathBuf } }` threaded through `serve()` and `AppState`/`SecurityConfig::build` so the allowlist can derive the right origins when the user brings their own cert (the user's cert presumably covers `daemon.foo.ts.net` or similar — we need that in allowed_hosts).
3. When both `--tls-cert` and `--tls-key` are present, skip `ensure_cert` entirely. Load via `rustls_pemfile` directly. Don't write `web.cert.pem` / `web.key.pem`.
4. Preserve rotation semantics: `ensure_cert` still handles SAN drift for the self-signed path. Drift now triggers on LAN-IP change (moving between networks) — this is the pain point for phone TOFU pinning (see "Risks" below).

**Estimate:** 1-2 days. Signature changes ripple into `security.rs::SecurityConfig::build` and the CLI. The test matrix is: self-signed-loopback, self-signed-LAN, user-provided-valid, user-provided-cert-key-mismatch, user-provided-missing-file.

**Hidden risk:** the user-cert path needs to validate that the cert and key actually match (see itr#228 — already filed). Without that, a typo on either flag silently serves a broken pair.

### itr#271 — Pairing token module + ephemeral LAN listener (blocks #272)

**This is the largest and most underestimated item in the chain.** The issue text undersells several design questions:

**Surface:** new module in `wisphive_web` (call it `pairing.rs`), new routes in `lib.rs::build_router`, gate extensions in `security.rs`, new protocol variant in `wisphive_protocol` for `DevicePaired`.

**Real work breakdown:**

1. **Pairing-token state machine** (~150 LOC):
   - In-memory `HashMap<TokenHash, Armed>` where `Armed = { created, expires, listener_handle, claimed_at: Option<Instant> }`.
   - Compare-and-swap claim semantics (race-safe single-use). Pattern already exists in `auth.rs::LoginThrottle::try_begin_attempt` — reuse the shape.
   - TTL reaper task (tokio interval) that drops expired entries AND shuts down the associated listener.

2. **Ephemeral second listener** (~100 LOC, but architecturally load-bearing):
   - `axum_server::bind_rustls(lan_addr, tls_config).handle(handle)` → spawn on a task → store `handle` in the `Armed` entry. Graceful shutdown via `handle.graceful_shutdown(Some(Duration::from_secs(5)))` on expire/cancel/claim.
   - The second listener must use the SAME cert as the primary (so the phone sees a consistent fingerprint once #270 lands). But its router needs a DIFFERENT `SecurityConfig`: the allowlist must accept the LAN origin, and `path_requires_device_token` must exempt `/pair` and `/api/pair/register` (token-gated instead of bearer-gated).
   - Tear-down races: if a phone is mid-`/api/pair/register` when the TTL fires, either let the request complete (graceful shutdown) OR reject it cleanly. Don't cut the socket mid-response.
   - At-most-one invariant: arming while another token is active either rejects or replaces. Per the issue, "at most one ephemeral listener" → reject.

3. **Three new routes:**
   - `POST /api/pair/arm` — bearer-gated. Should be sudo-gated (see itr#257 for the pattern) because it opens a LAN port.
   - `POST /api/pair/register` — token-gated (pairing token in URL), rate-limited (reuse `LoginThrottle`). Returns a new device token on success.
   - `POST /api/pair/cancel` — bearer-gated. Invalidates token, triggers listener shutdown.

4. **`DevicePaired` WS event:** new variant on `wisphive_protocol::ServerMessage`. Broadcast from the pairing module into the daemon's existing `tokio::sync::broadcast` via a new IPC message (or directly if the pairing module runs in-process with the daemon — which it does when `--web` is set).

5. **Audit events:** arm, claim, cancel, expire — all to `web_audit` via existing `append_web_audit`. Operators need this for forensics.

**Estimate:** 2-3 solid days including tests. The listener lifecycle is where subtle bugs hide; budget an integration test that arms + connects + tears down and asserts the port is released.

**Hidden risk — the big one:** we're running a second TLS listener with a different gate policy on a LAN-facing port. If the gate is wrong — if `/pair` leaks any bearer-gated endpoint — we've just shipped a LAN-exposed daemon surface. The mitigation is (a) a separate `Router` built from scratch for the ephemeral listener, not a clone of the primary, and (b) an integration test that enumerates every primary route and asserts the ephemeral listener responds 404/403 on all of them. Do not skip this test.

### itr#219 — WebAuthn register/login flows (parallel to #270/#271, required for #272)

> **SPLIT 2026-05-16** into umbrella #219 + sub-issues:
> - **#310** AuthProfile module (precursor; blocks #311)
> - **#311** PR-4: backend WebAuthn handlers consuming AuthPolicy
> - **#312** PR-5: frontend hooks + Login.tsx (closes #269 when it lands)
> - **#313** Enterprise device-enroll flow (separate; blocked by #310 + #311 + #220)
> - **#220** absorbs Devices.tsx 'enroll another passkey' + passkey list UI (was previously bundled here)
>
> See those issues for current scope. The breakdown below describes the original bundled shape and is kept for context.

**Real work breakdown:**

1. **Webauthn config construction.** `webauthn-rs::WebauthnBuilder` needs an RP ID (the registrable domain) and an RP origin (the full URL). RP ID is the big design question — see below.
2. **Challenge state store.** `webauthn-rs` hands back `PasskeyRegistration` / `PasskeyAuthentication` state blobs that must round-trip to `/finish`. In-memory `HashMap<ChallengeId, StateBlob>` with TTL. Similar shape to the pairing-token module.
3. **DB plumbing.** Already done: `insert_web_passkey`, `list_web_passkeys_for_device`, cascade-delete. Just need to call it.
4. **Frontend.** `usePasskey` hook, WebAuthn API calls (`navigator.credentials.create/get`), error handling for unsupported browsers, the Devices.tsx "enroll passkey" button.
5. **Chrome + Firefox + Brave smoke** on at least one Android phone. Needs a real device, cannot be cargo-tested. (Safari / iOS explicitly out of scope for v1 — see "Cross-cutting design decisions.")

**Estimate:** 3-4 days. The RP ID design is the wildcard; if we need to negotiate with mDNS or settle for per-origin credentials, add 1-2 days.

**Hidden risk — the RP ID problem:**

WebAuthn credentials are bound to an RP ID (the "site"). The spec allows RP ID to be a registrable suffix of the origin's effective domain — it DOES NOT allow IP addresses. Our production origins are:

- `https://localhost:3100` (loopback)
- `https://127.0.0.1:3100` (loopback)
- `https://192.168.1.42:3100` (LAN, for mobile pairing)
- `https://daemon.foo.ts.net` (optional trusted-cert path)

Credentials registered under RP ID `localhost` can only be used on `localhost`. Same for `daemon.foo.ts.net`. The IP-based origin **cannot be an RP ID at all** — Safari and Chrome both reject it. So a passkey enrolled on the desktop (origin `localhost`) is not usable from the phone (origin `192.168.1.42`), and vice versa.

**Mitigations, in increasing order of investment:**

- **A. Use `<hostname>.local` (mDNS) as RP ID across both origins.** If both desktop and phone can resolve `wisphive.local`, credentials work cross-origin. macOS resolves `.local` natively via Bonjour; iOS does too. Android and Linux don't always. This is the default guidance in the issue context.
- **B. Require the trusted-cert path (Tailscale, etc.) for credential portability.** Users on the self-signed path enroll one credential per origin, which is fine because the phone only needs its own credential. Document this explicitly.
- **C. Mint a separate credential during pairing, bound to the LAN origin's RP ID.** This is what #272's WebAuthn flow should actually do — the phone registers a credential scoped to the pairing origin, and we accept that as a "device passkey" stored in `web_passkeys`. Desktop passkey (enrolled in #269) stays separate.

**LOCKED (2026-04-23): strategy C — per-origin credentials.** Each device enrolls its own passkey scoped to whatever RP ID makes sense at its pairing origin. Desktop passkey and phone passkey are independent `web_passkeys` rows. Users who want one-credential-across-origins take the trusted-cert path (Tailscale, mkcert). Strategies A (mDNS) and B (require trusted cert) are documented as escape hatches but not the default.

> **SUPERSEDED 2026-05-16** by the AuthProfile reframe (see "Profiles" section below + itr#310). Strategy C as written silently broke for the LAN-IP-origin case — "per-origin credentials" has no resolution when the origin is an IP literal (WebAuthn forbids IP literals as RP IDs). The resolution is now profile-driven: under **LocalLAN**, `policy.rp_id_for_origin(lan_ip_origin) == None` and the phone authenticates via a device bearer (no passkey on LAN-IP origin). Under **Enterprise**, a real registrable domain is required at startup, so the IP-RP-ID problem is dodged entirely. See itr#310 and the "Profiles" section.

**Action item:** document the per-origin behavior in a comment where the RP ID is constructed in `auth.rs`, so future maintainers don't wonder why desktop and phone credentials are separate rows.

### itr#272 — Frontend add-device QR + mobile /pair (blocks nothing — this is the finish line)

**Surface:** new desktop route `/settings/add-device`, new public route `/pair`, QR rendering, WebAuthn browser API integration.

**Real work breakdown:**
1. `qrcode` npm dep (or `react-qr-code` — pick one and pin). Bundle-size check: `qrcode` is ~50 KiB gzipped, `react-qr-code` is smaller. Prefer the smaller unless we need raster output.
2. Desktop page: arm → render QR → subscribe to WS `DevicePaired` event → show success. Cancel button. Countdown timer matching backend TTL (with skew tolerance).
3. Mobile `/pair` route: bypasses bearer gate (uses URL token). Reads token from URL, posts to `/api/pair/register` with WebAuthn payload, shows success/failure states.
4. Error states: expired token (410), already-claimed (409), WebAuthn unsupported (older Android WebView), secure-context refused (iOS on self-signed cert with "proceed anyway" override).
5. Mobile-responsive CSS from day one. (User preference: never treat mobile as a follow-up.)

**Estimate:** 2 days.

**Safari / iOS is out of scope for v1** (locked 2026-04-23). Target browsers are Chrome, Firefox, and Brave. Chromium-based browsers handle self-signed overrides more predictably for WebAuthn enrollment than Safari's SecureContext semantics, so dropping Safari removes a whole class of iOS-specific failure modes from the risk list. Acceptance smoke runs on Android Chrome/Firefox/Brave. Safari support can be revisited post-GA if user demand shows up.

## Profiles (added 2026-05-16, see itr#310)

After the /alignment session on 2026-05-16, auth/security posture is selected at startup via an `AuthProfile` rather than a single global lock. This supersedes the original "strategy C" decision for #219, and reshapes what #271/#272/#220 do.

**Implementation:** `crates/wisphive_web/src/auth_profile.rs` (itr#310, landed Sprint-1 W1). Exposes `enum AuthProfile { LocalLAN, Enterprise { rp_id, rp_origin } }`, `struct AuthPolicy { ... }`, `policy.rp_id_for_origin(&Url) -> Option<RpId>`, and `validate_enterprise_config` for CLI fail-fast. Threaded through `AppState` + `SecurityConfig::build`. Frontend reads the active profile via `GET /api/auth/profile` (origin-aware, unauth, gated by Origin/Host allowlist; bypasses both the device-token gate and the setup-required gate).

**LocalLAN** (default — opinionated for local-first deploys):
- TLS: self-signed OK
- Phone passkey: **none** — phone authenticates via device bearer issued at pairing (sidesteps WebAuthn's IP-literal-as-RP-ID prohibition)
- Desktop passkey: optional convenience (origin = `localhost`, RP ID = `localhost`, works fine)
- Ephemeral LAN pairing listener: enabled (#271/#272 active)
- Sudo gate on passkey-register: no
- UV requirement: Preferred
- Login throttle threshold: 5 fails (existing)
- RP ID derivation: `Some("localhost")` for loopback origins, `None` for RFC1918 IP literals

**Enterprise** (operator-provided cert + real domain):
- TLS: user-provided cert required (`--tls-cert`, `--tls-key`)
- Phone passkey: yes (RP ID = `--auth-rp-id`, valid registrable domain, works cross-device)
- Desktop passkey: optional
- Ephemeral LAN pairing listener: **disabled** (#271/#272 dormant under this profile; `/api/pair/arm` returns 409)
- Sudo gate on passkey-register: yes
- UV requirement: Required
- Login throttle threshold: 3 fails (stricter than LocalLAN)
- RP ID derivation: always `Some(--auth-rp-id)`
- Device-add: operator-driven enroll URL on the primary listener (itr#313), not QR/LAN

**Profile-switch caveat:** existing `web_passkeys` rows store their RP ID. Switching profiles invalidates credentials whose stored RP ID doesn't match the new profile — daemon logs WARN per mismatched row at startup; operator must re-enroll.

CLI selection: `wisphive daemon start --web --auth-profile {local-lan|enterprise}` (default `local-lan`). Enterprise additionally requires `--tls-cert`/`--tls-key`/`--auth-rp-id` with fail-fast validation. Frontend learns the active profile via `GET /api/auth/profile` (origin-aware, unauth, gated by Origin/Host allowlist).

Atomic profiles only in v1: no per-knob env overrides. Add profiles sparingly when a real new posture emerges; avoid knob-by-knob configs.

## Cross-cutting design decisions

These aren't captured in any single issue but will bite if we skip them:

1. **RP ID: per-origin credentials (strategy C above).** LOCKED 2026-04-23; **superseded by AuthProfile 2026-05-16**. Under LocalLAN: per-origin where possible, none on LAN-IP. Under Enterprise: single static RP ID from `--auth-rp-id`. See "Profiles" section above and itr#310.
2. **Browser support: Chrome, Firefox, Brave.** LOCKED 2026-04-23. Safari / iOS Safari out of scope for v1. Target Android Chrome/Firefox/Brave for mobile smoke; desktop smoke on Chrome/Firefox/Brave.
3. **Cert regeneration on network change is a user-visible event.** Moving between WiFi networks flips the SAN set, regenerates the cert, and breaks any phone that pinned the old fingerprint. Document this. The trusted-cert path (Tailscale, mkcert) is the long-term answer.
4. **Ephemeral listener is LAN-only, always.** Never bind to `0.0.0.0`. Bind to the detected primary LAN IP. If we can't detect one (laptop on airplane), fail the arm with a clear error.
5. **One ephemeral token at a time.** `POST /api/pair/arm` rejects 409 if a token is already armed. Simpler than multi-pairing state and removes a whole class of race.
6. **Sudo-gate `/api/pair/arm`.** Opening a LAN port is a trust-elevating action. Reuse the pattern from itr#257.
7. **Shared TLS cert between primary and ephemeral listeners.** Different ports, same cert — the phone pins one fingerprint regardless of which flow re-used it.

## Security prerequisites

These must close before we expose any LAN listener in production. None block development, but all block the milestone:

- **itr#79** — dev-mode CORS currently `Any` on config endpoints. Not LAN-relevant directly but surfaces a malicious-page-attacks-dev-daemon vector that we don't want live while mobile is the hot feature.
- **itr#80** — hook fail-open is exploitable via induced errors. Not web-facing but adjacent to any release push.
- **itr#257** — sudo-gate `/api/devices/{id}/revoke` + rate-limit. The pairing flow mints new device tokens — we need the revocation path hardened before operators actually use it.

Nice-to-have before GA (not blockers):
- itr#219 lands (hard blocker for full mobile flow, but pairing can ship with password-auth-only as a temporary state if #219 slips).
- itr#274 (web auth token storage — localStorage XSS exfiltration risk) — not a blocker, but mobile surface amplifies the impact.

## Acceptance matrix

Split by layer because `cargo test` can't cover half of this:

### Automated (cargo / vitest)

- [ ] `#227`: `compute_sans` filters named interfaces + CGNAT; injection-based unit tests.
- [ ] `#270`: user-provided cert path round-trips; SAN drift on LAN-IP change triggers regen; cert/key mismatch errors cleanly.
- [ ] `#271`: token compare-and-swap (no double-claim); TTL reaper releases listener port; ephemeral router rejects every primary route; `DevicePaired` event broadcasts.
- [ ] `#219`: register/login start/finish happy path; challenge TTL; replay rejection; device-scoped credential binding.
- [ ] `#272`: QR render, countdown, cancel, expired-state rendering.

### Browser smoke (manual, before tagging the release)

Target browsers: **Chrome, Firefox, Brave** (desktop + Android). Safari / iOS out of scope for v1.

- [ ] Fresh install → browser auto-opens → set password → land on dashboard (Stage 1 end-to-end). Desktop: Chrome, Firefox, Brave.
- [ ] Click "Add Device" → QR appears → Android Chrome scans → proceed past cert warning → passkey enroll → success → desktop shows `DevicePaired`.
- [ ] Same flow, Android Firefox.
- [ ] Same flow, Android Brave (Chromium-based, should be equivalent to Chrome — one-pass smoke is fine).
- [ ] Cancel from desktop mid-scan → phone gets clear error.
- [ ] Expire during scan → phone gets 410, desktop UI shows expired banner.
- [ ] Network change between pairings: verify cert-fingerprint prompt on phone, document the re-accept step.
- [ ] Revoke paired phone from desktop → phone's WS disconnects → `/api/me` returns 401.

### Trusted-cert path (nice to have pre-GA, required for friction-free UX)

- [ ] `--tls-cert` / `--tls-key` with a Tailscale cert → phone hits `https://daemon.foo.ts.net` → zero warnings → passkey enroll works.

## Milestone sequencing

Suggested PR cadence — each PR is reviewable on its own and delivers a closable itr:

1. **PR 1** (itr#227): SAN filter. Half-day. Unblocks #270.
2. **PR 2** (itr#270): LAN IP SAN + user-cert flags. 1-2 days. Unblocks #271 + #272.
3. **PR 3** (itr#269): Frontend onboarding route (Stage 1c). Can land anywhere after #268 closes — reviewer-friendly to ship independently. 1-1.5 days.
4. **PR 4** (itr#219 part 1): WebAuthn handler scaffolding + register/login start/finish backend + challenge store. 2 days.
5. **PR 5** (itr#219 part 2): frontend passkey hooks + Devices.tsx enroll UI + RP ID decision locked in. 1-2 days.
6. **PR 6** (itr#271): pairing module + ephemeral listener + `DevicePaired` event. 2-3 days. **This PR needs the "no primary routes on ephemeral listener" integration test.**
7. **PR 7** (itr#272): desktop QR + mobile `/pair` + browser smoke. 2 days + an afternoon on real devices.

Realistic ceiling: **~2.5 weeks of focused single-dev work**, including the browser smoke and the RP ID design call. Security prerequisites (#79, #80, #257) can run in parallel if a second dev is available.

With Safari dropped and strategy C locked, the worst-case RP ID fallback budget is reclaimed — ~2.5 weeks is the realistic ceiling.

## Open questions

1. ~~**Do we ship (C) — per-origin credentials — without trying to solve portability?**~~ **Resolved 2026-04-23: yes, strategy C.** Trusted-cert path is the portability answer for users who want one-credential-across-origins.
2. ~~**Browser support scope?**~~ **Resolved 2026-04-23: Chrome, Firefox, Brave. Safari / iOS out of scope for v1.**
3. **Should `/api/pair/arm` require sudo reauth?** Recommendation: yes. Mirrors itr#257's pattern for revoke.
4. **Do we want a second ephemeral listener on `[::1]` for IPv6-only LANs?** Recommendation: defer. IPv4 RFC1918 covers the 99% case.
5. **What's the pairing token TTL?** Issue says ~10 min; feels right. Make it configurable via env var so operators behind slow WiFi can extend.
6. **Do we need a `wisphive web pair` CLI as a fallback for headless setups?** Recommendation: defer until a user asks — the QR flow assumes a GUI browser, which is the realistic onboarding path.

## Related docs

- [plan-cross-agent-conflict-gate.md](plan-cross-agent-conflict-gate.md) — unrelated workstream, similar plan-doc structure.
- [open-source-path.md](open-source-path.md) — OSS positioning; the mobile flow is a top-3 demo story.
- itr#266 — the existing umbrella issue. This doc expands its scope with sizing + design decisions.
