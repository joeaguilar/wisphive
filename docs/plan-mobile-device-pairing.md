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

## LocalLAN browser smoke procedure (itr#315)

Reproducible desktop smoke for the LocalLAN profile. Run after any change that touches `auth_profile.rs`, `auth.rs`, `passkey.rs`, the four `/api/auth/passkey/*` routes, `Login.tsx`, `useAuth.ts`, `useAuthProfile.ts`, or `usePasskey.ts`. Closes itr#219 (umbrella) + itr#269 (passkey onboarding) once green on all three browsers.

Enterprise smoke is a separate procedure tracked under itr#316; this section is LocalLAN-only because itr#270 (`--tls-cert`/`--tls-key`) has not shipped and Enterprise selection requires it.

### 0. Prerequisites

- Rust toolchain installed (`rustup show` reports a toolchain compatible with edition 2024 — see `CONTRIBUTING.md`).
- Wisphive binary built and on PATH: from repo root, run `./install.sh` (builds release, installs to `~/.cargo/bin`, codesigns on macOS).
- Clean `~/.wisphive/` directory. **If you have prior daemon state, back it up first:**
  ```bash
  wisphive emergency-off || true                         # stop the daemon
  mv ~/.wisphive ~/.wisphive.bak-$(date +%Y%m%d-%H%M%S)  # back up
  ```
  A truly fresh first-run is the only way to exercise the `phase === "setup"` UI and the post-set-password `authed-pending-enroll` gate.
- All three target browsers installed (current stable):
  - Google Chrome
  - Mozilla Firefox
  - Brave
- macOS: Touch ID enrolled (System Settings → Touch ID & Password). Windows: Windows Hello configured. Linux: have a USB security key ready (FIDO2/CTAP2, e.g. YubiKey 5).

### 1. Daemon startup

Default profile is LocalLAN — no `--auth-profile` flag required.

```bash
wisphive daemon start --web --no-open
```

`--no-open` keeps the smoke driver in control of when each browser opens, so the same fresh `https://localhost:3100` load can be repeated across three browsers without auto-launching the OS default.

**Expected log output** (key lines):

- A `serving https://localhost:3100` (or similar) line confirming the bound port.
- No WARN about `passkey RP ID does not match active profile` (a fresh `~/.wisphive/` has no `web_passkeys` rows for `scan_passkey_rp_id_drift` to scan; a WARN here means you skipped the backup step).
- No error about `--auth-rp-id` (LocalLAN ignores it; if you accidentally passed `--auth-profile enterprise` the daemon would have failed fast with `MissingTlsFlags` before logging anything).

**Expected files in `~/.wisphive/`** (auto-provisioned on first run):

- `wisphive.sock` — Unix socket
- `wisphive.pid` — daemon PID
- `wisphive.db` — SQLite state (empty `web_devices` / `web_passkeys` tables)
- `mode` — `active`
- `web.cert.pem` — self-signed ECDSA P-256 leaf cert (397-day validity cap)
- `web.key.pem` — private key, mode `0600`
- `web.cert.meta.json` — SAN sidecar for drift detection

There should be **no** `web.token` file — per-device bearer tokens live in browser `localStorage`, not on disk.

### 2. First-run set-password flow

For each browser, open `https://localhost:3100`.

> **Use `localhost`, not `127.0.0.1` or `[::1]`.** WebAuthn forbids
> IP-literal RP IDs at the browser layer (§5.1.3 step 9), so the
> passkey ceremony cannot complete on `https://127.0.0.1:3100` —
> Chrome throws `SecurityError: This is an invalid domain` the moment
> the user clicks "Enroll passkey". The daemon does a `308` redirect
> from `127.0.0.1` / `[::1]` browser navigation to `localhost`
> automatically (see `security::loopback_ip_redirect`), so most users
> never land on an IP-literal URL. If the redirect is bypassed
> (e.g. you pasted directly into a script that doesn't follow 3xx)
> you'll see `can_enroll_passkey_on_this_origin: false` in
> `/api/auth/profile` and the SPA will hide the enroll affordance —
> the honest no-passkey-here answer rather than a button that fails
> on click.

**TLS warning + accept-and-continue per browser:**

- **Chrome / Brave**: "Your connection is not private" page → click "Advanced" → "Proceed to localhost (unsafe)". Or type `thisisunsafe` anywhere on the warning page (no input field; just type with the page focused). The page reloads through the warning.
- **Firefox**: "Warning: Potential Security Risk Ahead" → "Advanced…" → "Accept the Risk and Continue".

**Expected UI:**

- Header: "wisphive" (`login-title`).
- Subtitle: "Welcome. Set a password to finish setup."
- Fields: New password, Confirm password, Device name (prefilled like `Mac (Chrome)` or `Windows (Firefox)` — `defaultDeviceName()` parses the UA).
- Submit button: "Set password".
- Below-form button: "I set it in a terminal — reload" (calls `refreshStatus`; not used in this smoke).

**Password constraints surfaced:**

- Backend enforces `MIN_PASSWORD_LEN = 8`; the frontend mirrors it. Try a 7-character password and confirm the inline error reads "Password must be at least 8 characters." with no round-trip to the server.
- Mismatched confirm shows "Passwords do not match." (also client-side).

**Success path** (use an 8+ char password + matching confirm):

- Submit fires `POST /api/auth/set-password`.
- The daemon mints a device row + bearer token, writes a `set_password_succeeded` audit row, and returns the bearer.
- The frontend stashes the bearer under `wisphive-web-token` in `localStorage` and `useAuth` transitions phase. Because LocalLAN + `https://localhost:3100` resolves to RP ID `localhost` (`AuthPolicy::rp_id_for_origin` returns `Some("localhost")`), `canEnrollPasskeyOnThisOrigin === true`, and the transient `authed-pending-enroll` phase fires.

### 3. Post-set-password enroll-passkey flow (itr#312 happy path)

Driven by `phase === "authed-pending-enroll"` (the new `AuthPhase` from `b6662b2` that fixed the M1 race in the #312 review). On origins where `canEnrollPasskeyOnThisOrigin` is false, `useAuth` skips the transient phase entirely and goes straight to `authed` — so the card below MUST appear on `https://localhost:3100`. If it doesn't, M1 has regressed.

**Expected UI:**

- Header: "wisphive".
- Subtitle: "Set up a passkey on this device?"
- Body: "Passkeys let you sign in with Touch ID, Windows Hello, or a security key instead of typing your password."
- Primary button: "Enroll passkey".
- Secondary button: "Skip for now".

**Click "Enroll passkey":**

- Frontend calls `POST /api/auth/passkey/register/start`. Response is the flattened body `{ session_id, publicKey }` (#311 review note 1) — the hook strips `session_id` and passes `{ publicKey }` to `navigator.credentials.create()`.
- **OS dialog fires** with multiple options. For v1 LocalLAN smoke, **pick the local-device option** (NOT the QR-code option):
  - macOS: Touch ID prompt ("Use your Touch ID to sign in to localhost?"). DO NOT pick "Use another device".
  - Windows: Windows Hello prompt (PIN / face / fingerprint). DO NOT pick "Use a different device".
  - USB security key: "Insert your security key and touch it." (see §7 known limitation about non-discoverable USB credentials).
  - **QR-code / cross-device hybrid** (caBLE): the OS dialog also offers "Use another device" / "Pair with phone" which displays a WebAuthn-native QR code. **This is NOT Wisphive's phone-pairing flow** — it's the browser's native cross-device WebAuthn, which expects your phone to create a credential synced to its OS password manager (iCloud Keychain / Google Password Manager). In v1 this path is unsupported: scanning the QR with a phone will typically either fail registration outright or create a credential the daemon can't use. Wisphive's own phone-pairing flow ships under itr#283 epic (itr#271 pairing token + itr#272 phone `/pair` route), which is NOT in this sprint. **Do not test the QR option as part of this smoke** — pick the local-device option only.
- On user verification: frontend calls `POST /api/auth/passkey/register/finish`, daemon inserts a `web_passkeys` row (with `rp_id = "localhost"`), `handleEnrollPasskey` calls `onCompleteEnrollGate()`, `useAuth` flips to `authed`, App.tsx unmounts Login, dashboard appears.

**Verify:**

- `~/.wisphive/wisphive.db` now has exactly one `web_passkeys` row whose `rp_id` column is `"localhost"`:
  ```bash
  sqlite3 ~/.wisphive/wisphive.db 'SELECT id, device_id, rp_id FROM web_passkeys;'
  ```
- Restart the daemon (`wisphive emergency-off && wisphive daemon start --web --no-open`) and confirm NO WARN log about `passkey RP ID does not match active profile`.

### 4. Logout + login-with-passkey flow

To exercise the login path with the credential just enrolled, either:

- **Open an incognito window** to the same `https://localhost:3100` (preferred — keeps the original tab's `localStorage` intact for comparison), OR
- **Clear `wisphive-web-token` from `localStorage`** (DevTools → Application → Local Storage → `https://localhost:3100`) and reload.

You'll be back on the login form, this time on `phase === "unauthed"` (not `setup` — password is already set).

**Expected UI:**

- Header: "wisphive".
- Subtitle: "Sign in to review pending decisions."
- **NEW**: "Sign in with a passkey" button rendered **above** the password form, inside a `.login-passkey-cta` block, followed by a divider reading "or use your password". This affordance is gated on `showPasskeyAffordances = profile.loaded && profile.canEnrollPasskeyOnThisOrigin` — must be true on `https://localhost:3100`.
- Below: the password form (Password, Device name, "Sign in" — Confirm password absent because `isSetup === false`).

**Click "Sign in with a passkey":**

- Frontend calls `POST /api/auth/passkey/login/start` (only on click — never on mount, per #311 review note 7).
- OS dialog fires (Touch ID / Hello / security key), same authenticator as enrollment.
- On UV success: frontend calls `/finish`, daemon returns `{ token, device_id, enrolling_device_id }` (`enrolling_device_id` is `Some(...)` on passkey login per #311 review note 2). Frontend stashes the new bearer; App.tsx re-renders authed.

**Password form remains as fallback.** Typing the password and clicking "Sign in" must still work even with the passkey button present.

### 5. Edge cases to exercise per browser

Run each of these once per browser. Capture screenshots of any visible mismatch.

#### 5.1 LAN-IP origin (must NOT show passkey affordances)

Find your machine's LAN IP (`ipconfig getifaddr en0` on macOS, `ip route get 1` on Linux). Open `https://192.168.x.y:3100` in the same browser.

- Accept the TLS warning (the cert SAN includes `localhost` only by default until itr#270; expect a name-mismatch warning on top of the self-signed warning).
- On the login form: the "Sign in with a passkey" button MUST be absent. The post-set-password enroll card MUST be absent (you'd never reach it from an LAN-IP origin on a freshly-bootstrapped daemon, but if you're testing on a re-used daemon and the bearer is unset, only the password form should appear).
- Why: `AuthPolicy::rp_id_for_origin` returns `None` for RFC1918 IPv4 origins under LocalLAN (`auth_profile.rs::loopback_rp_id_from_origin`), `/api/auth/profile` returns `can_enroll_passkey_on_this_origin: false`, `useAuthProfile` sets `canEnrollPasskeyOnThisOrigin = false`, `Login.tsx` hides both passkey UIs.
- Password login must still work from the LAN-IP origin (subject to the Origin/Host allowlist letting the request through — it should under LocalLAN).

#### 5.2 Throttle banner (shared bucket, password + passkey)

The `LoginThrottle` (in `auth.rs`) is per-IP and shared between password and passkey paths (#311 review note 4). The backoff schedule starts immediately on the first failure (250ms → 30s ceiling), and a sustained run of failures past `login_throttle_threshold = 5` (LocalLAN) climbs the schedule.

- Submit 5 wrong passwords in rapid succession (or mix in failed passkey attempts — same bucket).
- Expect a 429 with a `Retry-After` header. The login form renders a red `.login-error-throttled` banner reading **"Too many attempts — try again in Ns."** with `N` counting down (driven by `useAuth`'s `retryAfter` and the `setInterval` in `Login.tsx`).
- Wait out the countdown → form re-enables → next correct password succeeds.
- Critical UX detail: the throttle copy must NOT mention "wrong password" specifically (passkey failures also feed it). The current copy is generic — verify it stays that way.

#### 5.3 Skip enroll path

On a fresh `~/.wisphive/`, complete the set-password flow but click "Skip for now" instead of "Enroll passkey".

- `handleSkipEnroll` calls `onCompleteEnrollGate()`; phase flips to `authed`; dashboard appears.
- No row added to `web_passkeys`.
- Sanity: `sqlite3 ~/.wisphive/wisphive.db 'SELECT COUNT(*) FROM web_passkeys;'` returns `0`.
- Logout and reload: the "Sign in with a passkey" button MUST be absent on the unauthed form, because there are no credentials enrolled. (This is currently a frontend gating decision — the button shows whenever the origin can host enroll; verify the behavior matches your build's `Login.tsx`.)

#### 5.4 Re-enroll attempt (known UX gap)

After enrolling a passkey, logout, and use the password form to log back in. Wisphive does not yet expose a logged-in "Manage passkeys" surface — that's itr#220. The current build has no in-flow re-enroll path, so this edge case is exercised indirectly:

- Try to enroll the same authenticator a second time via DevTools (replay `POST /api/auth/passkey/register/start` + finish from the network panel) or wait for itr#220's UI.
- Expected backend error: "this credential already exists for this user on this authenticator" or similar.
- Current frontend surfaces this as `PasskeyError { kind: "unknown" }` (itr#322 S8: refine the `InvalidStateError` taxonomy). **Flag this in the smoke results as a UX gap; do NOT fail the smoke** — the error path is functionally correct, just unfriendly.

### 6. Per-browser quirks

- **Chrome / Brave** (Chromium): Touch ID (macOS), Windows Hello, USB key all work via the native `navigator.credentials` path. Brave behaves identically to Chrome here — no Shields exception needed for the localhost origin.
- **Firefox**: Same authenticator coverage; resident-key-only by design on our backend (#311). Firefox's WebAuthn dialog wording differs from Chrome's ("Use a device") but the OS prompt that fires after is identical.
- **Safari**: **EXPLICITLY OUT** for v1 per itr#283 epic. Do not run smoke against Safari, do not file Safari bugs from this smoke. itr#283 will revisit post-GA.

### 7. Common failure modes & fixes

- **TLS warning blocks the page entirely** (Chrome / Brave show a hard block on some flags):
  - Accept-and-continue: "Advanced" → "Proceed". If the link is missing, focus the page and type `thisisunsafe`.
  - Firefox: "Advanced…" → "Accept the Risk and Continue".
  - If the cert is genuinely expired or corrupted, delete `web.cert.pem` / `web.key.pem` / `web.cert.meta.json` from `~/.wisphive/` and restart the daemon — `ensure_cert` will regenerate.
- **"Sign in with a passkey" button missing** on `https://localhost:3100`:
  - Open DevTools → Network → reload → inspect `GET /api/auth/profile`. If the body's `can_enroll_passkey_on_this_origin` is `false`, the origin doesn't match LocalLAN's loopback gate. Verify you're on `localhost` / `127.0.0.1` / `[::1]` and NOT an RFC1918 IP. The gate is set by `AuthPolicy::rp_id_for_origin` in `auth_profile.rs`.
  - If `can_enroll_passkey_on_this_origin` is `true` but the button is still hidden, the frontend probe (`useAuthProfile`) may have errored. Check the console for a fetch error and verify the bearer is set (`localStorage.getItem('wisphive-web-token')`).
- **Post-set-password enroll card never appears**:
  - This is the M1 regression from #312 review. The fix promoted the gate from a `Login.tsx`-local `pendingEnroll` state into `useAuth`'s `authed-pending-enroll` phase. If the card is missing, `useAuth.setPhase("authed-pending-enroll")` is not firing on `setPassword` success — check `useAuth.ts` for a regression and re-read `crates/wisphive_web/frontend/src/components/Login.tsx` lines 120–128.
- **Touch ID / Windows Hello not prompting** when "Enroll passkey" is clicked:
  - macOS: System Settings → Touch ID & Password — confirm at least one fingerprint enrolled. Chrome/Brave/Firefox each need site permission to use the authenticator (System Settings → Privacy & Security → Accessibility / Input Monitoring).
  - Windows: Settings → Accounts → Sign-in options → Windows Hello — confirm setup.
  - USB key: re-seat the key. Some keys require touch (gold button) within a window; if you miss it, the dialog times out and surfaces as `PasskeyError { kind: "cancelled" }`.
- **401 after enroll** (the credential round-trips fine but the next bearer-gated call rejects):
  - This points at the device-row semantics gap tracked under itr#319. The new device row created during enroll has no passkeys of its own; passkey login returns `enrolling_device_id` pointing at the original device — UI surfaces that consume the bearer must not stale-cache `device_id` ↔ `passkey_id`.
- **`POST /api/auth/passkey/login/start` fires on page-load** (devtools shows it before any click):
  - Don't do this — it consumes a throttle slot and inserts a `ChallengeStore` row that reaps at 60s cadence (#311 review note 7). The current `usePasskey.loginWithPasskey` only fires on user click; if you see auto-fires, the hook has regressed.
- **USB security key enrolls but login-with-passkey can't find it** (known v1 limitation — tracked under itr#321):
  - webauthn-rs 0.5's `start_passkey_registration` hardcodes `require_resident_key(false)`, so USB keys MAY enroll a non-discoverable credential. The login flow uses `start_discoverable_authentication`, which only sees resident credentials. Platform passkeys (Touch ID, Windows Hello, iOS keychain) create resident credentials regardless and work fine; USB-only authenticators may not.
  - **For LocalLAN smoke, use a platform passkey for the full round-trip.** If you specifically want to smoke a USB key, accept that the enroll-then-login flow may fail at login with `PasskeyError { kind: "server_rejected", message: "..." }` or an `unknown` kind. Document the failure mode in the result table but DO NOT count it as a sprint blocker — itr#321 captures the upstream/lower-level-API fix path.

### 8. What to capture per run

Per browser × per step, record: **browser + browser version + OS + pass/fail + screenshots of any failures or unexpected UI**. Suggested table to paste into the close-reason of #219:

| Browser | Version | OS         | 2. set-pwd | 3. enroll | 4. login-pk | 5.1 LAN-IP | 5.2 throttle | 5.3 skip | Notes |
|---------|---------|------------|------------|-----------|-------------|------------|--------------|----------|-------|
| Chrome  |         |            |            |           |             |            |              |          |       |
| Firefox |         |            |            |           |             |            |              |          |       |
| Brave   |         |            |            |           |             |            |              |          |       |

Use `pass` / `fail (link)` / `n/a (reason)` in cells. Attach screenshots in the linked issue notes; do not paste raw screenshots into the table.

### 9. Closing #219 and #269

Once the table is green across all three browsers:

1. Paste the completed table into the close-reason for itr#219:
   ```
   LocalLAN browsers passed; Enterprise smoke deferred to itr#316 pending #270.
   <table>
   ```
   `itr close 219 -m "<close-reason>"`
2. Close itr#269 mechanically:
   ```
   itr close 269 -m "Closed mechanically — #312 + #315 fulfilled passkey onboarding acceptance."
   ```
3. Any new bug surfaced during the smoke that is not already filed (itr#319 / #320 / #321 / #322) MUST be filed as a fresh `itr` issue with the exact repro step from this procedure that produced it.

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
