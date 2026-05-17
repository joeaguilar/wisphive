import { useCallback, useEffect, useState } from "react";
import type { AuthError, UseAuth } from "../hooks/useAuth";
import { useAuthProfile } from "../hooks/useAuthProfile";
import { usePasskey, type PasskeyError } from "../hooks/usePasskey";

interface Props {
  phase: UseAuth["phase"];
  error: AuthError | null;
  onLogin: UseAuth["login"];
  onSetPassword: UseAuth["setPassword"];
  onClearError: UseAuth["clearError"];
  onRefreshStatus: UseAuth["refreshStatus"];
}

/** Minimum password length for onboarding. Must match the backend
 * constant in `crates/wisphive_web/src/lib.rs::MIN_PASSWORD_LEN` — if
 * they drift the UI validation fails differently than the server and the
 * error gets blamed on the network. */
const MIN_PASSWORD_LEN = 8;

/** Render a `PasskeyError` as the inline user-visible string. Kept as a
 * pure function (not a sub-component) so the same mapping is used in
 * both the enroll-error and login-error surfaces, with no React lifecycle
 * surprises. Keeps the messages short and actionable — full server text
 * is preserved for `server_rejected` because those carry the load-bearing
 * messages (e.g. `"counter regression detected"`, throttle copy). */
function passkeyErrorText(error: PasskeyError): string {
  switch (error.kind) {
    case "unsupported":
      return "This browser does not support passkeys. Use a password instead.";
    case "cancelled":
      return "Passkey prompt was cancelled. Try again or use a password.";
    case "origin_unavailable":
      // Defensive — the UI should hide the button when
      // canEnrollPasskeyOnThisOrigin is false, but if the user races
      // a profile switch (or the gate ever drifts) we'd land here.
      return "Passkeys aren't available on this URL.";
    case "uv_failed":
      return "User verification failed on the authenticator. Try again or use a password.";
    case "server_rejected":
      // Show the server's message verbatim. The throttle copy
      // (`"throttled"`) and counter-regression (`"counter regression
      // detected"`) are both single-source-of-truth from the daemon
      // and must surface without paraphrase. Strip the bare word
      // `"throttled"` (the server's plain-text 429 body) — the
      // password form's countdown already shows the same condition
      // more usefully.
      if (error.message.trim() === "throttled") {
        return "Too many login attempts. Wait a moment and try again.";
      }
      return error.message;
    case "network":
      return error.message;
    case "unknown":
      return error.message || "Unknown passkey error.";
  }
}

/** Derive a friendly default device name so the list in Settings/Devices
 * is recognisable ("MacBook (Chrome)") instead of a random UUID prefix.
 * User can still override. */
function defaultDeviceName(): string {
  const ua = navigator.userAgent;
  const browser = /Firefox\//.test(ua)
    ? "Firefox"
    : /Edg\//.test(ua)
      ? "Edge"
      : /Chrome\//.test(ua)
        ? "Chrome"
        : /Safari\//.test(ua)
          ? "Safari"
          : "Browser";
  const platform = /iPhone|iPad/.test(ua)
    ? "iOS"
    : /Android/.test(ua)
      ? "Android"
      : /Macintosh/.test(ua)
        ? "Mac"
        : /Windows/.test(ua)
          ? "Windows"
          : /Linux/.test(ua)
            ? "Linux"
            : "Device";
  return `${platform} (${browser})`;
}

export function Login({
  phase,
  error,
  onLogin,
  onSetPassword,
  onClearError,
  onRefreshStatus,
}: Props) {
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [deviceName, setDeviceName] = useState(defaultDeviceName);
  const [submitting, setSubmitting] = useState(false);
  const [countdown, setCountdown] = useState(0);
  const [localError, setLocalError] = useState<string | null>(null);
  // Sub-phase that activates ONLY after `setPassword` succeeds and the
  // origin supports passkey enrollment. While `enrolling`, the password
  // form is replaced by the passkey enroll step. After enroll succeeds
  // OR is skipped, `pendingEnroll` flips false and the user's existing
  // token (set during setPassword) drives the app shell into the
  // authed view via the parent gate in App.tsx.
  //
  // We hold this as local state (rather than promoting it to useAuth)
  // because it's purely a Login.tsx UX concern: the user IS authed at
  // the moment we enter this state — they just haven't been shown the
  // dashboard yet. Pulling it into useAuth would either invent a new
  // `authed+enrolling` phase (with no other consumer) or leak Login's
  // step model into the app shell.
  const [pendingEnroll, setPendingEnroll] = useState(false);
  // Inline passkey-error region — separate from `error` (which is the
  // password-login channel) and `localError` (client-side validation).
  // Cleared on retry/skip so the user never sees stale text alongside
  // a fresh prompt.
  const [passkeyError, setPasskeyError] = useState<PasskeyError | null>(null);
  const [passkeyBusy, setPasskeyBusy] = useState<false | "enrolling" | "logging-in">(false);
  const isSetup = phase === "setup";
  const profile = useAuthProfile();
  const passkey = usePasskey();
  const showPasskeyAffordances = profile.loaded && profile.canEnrollPasskeyOnThisOrigin;

  // Clear localError whenever a fresh server error arrives (otherwise the
  // union render below keeps showing the stale client-side validation
  // message on top of a throttle countdown). Separate from the clear-on-
  // keystroke path because a user might submit without touching the
  // field again — e.g. clicking "I set it in a terminal — reload".
  useEffect(() => {
    if (error) setLocalError(null);
  }, [error]);

  // Reset localError on phase transitions (setup → unauthed from a 409).
  // The union render would otherwise leak a "Passwords do not match"
  // message onto the login form.
  useEffect(() => {
    setLocalError(null);
  }, [phase]);

  // Drive the 429 Retry-After countdown. We trust the server-supplied value
  // and tick it down locally — cheaper than polling /api/auth/status.
  const isThrottled = error?.kind === "throttled";
  useEffect(() => {
    if (!isThrottled || !error?.retryAfter) {
      setCountdown(0);
      return;
    }
    setCountdown(error.retryAfter);
    const interval = setInterval(() => {
      setCountdown((c) => {
        if (c <= 1) {
          clearInterval(interval);
          // Only clear the throttled error — a fresh error that arrived
          // mid-countdown (e.g. user typed something racy) must survive.
          // The effect's dep on `error` guarantees we see the latest.
          if (error?.kind === "throttled") onClearError();
          return 0;
        }
        return c - 1;
      });
    }, 1000);
    return () => clearInterval(interval);
  }, [error, isThrottled, onClearError]);

  const disabled = submitting || countdown > 0;

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (disabled || !password) return;
      setLocalError(null);
      if (isSetup) {
        // Client-side floor matches the backend MIN_PASSWORD_LEN so a
        // user with a weak password gets immediate feedback instead of
        // a round-trip to a 400.
        if (password.length < MIN_PASSWORD_LEN) {
          setLocalError(`Password must be at least ${MIN_PASSWORD_LEN} characters.`);
          return;
        }
        if (password !== confirm) {
          setLocalError("Passwords do not match.");
          return;
        }
      }
      setSubmitting(true);
      try {
        const ok = isSetup
          ? await onSetPassword(password, deviceName)
          : await onLogin(password, deviceName);
        // On success, wipe both password fields from React state
        // immediately — React GC eventually reclaims closure-captured
        // strings, but shortening the window is cheap belt-and-braces
        // against memory dumps and devtools inspection.
        //
        // On failure, the wipe decision depends on *why* it failed.
        // - "invalid" (wrong password, weak password) → wipe confirm to
        //   force a deliberate retry; keep the primary field so the
        //   user can correct a typo.
        // - network/throttled/server → keep both fields; the user's
        //   input is correct, the server just isn't cooperating yet.
        // A 409 from setPassword also flips the hook's phase to
        // unauthed — wipe the primary password there too because the
        // typed value is a *new* password, which is not the right
        // input for the login form the user now sees.
        if (ok) {
          setPassword("");
          setConfirm("");
          // Post-set-password gate: if this origin supports passkey
          // enrollment, intercept the transition to the dashboard so
          // the user can opt into a passkey on their way through.
          // `phase` is still `"setup"` here (the parent re-renders
          // immediately after `setPassword` returns true, but our
          // local state runs synchronously before that). We use
          // `isSetup` to disambiguate — only the setup flow leads to
          // the enroll step; password login (`!isSetup`) drops
          // straight to the dashboard like before.
          if (isSetup && showPasskeyAffordances) {
            setPendingEnroll(true);
          }
        }
        // Failure branches wipe defensively based on the outcome: the
        // updated `error` is committed by the time the next render runs,
        // but we read what's most recently-set via the effect below
        // rather than trying to read the post-setState value here.
      } finally {
        setSubmitting(false);
      }
    },
    [disabled, isSetup, password, confirm, deviceName, onLogin, onSetPassword, showPasskeyAffordances],
  );

  // Selective wipe on server failure. Driven off `error` changes so the
  // decision sees the newly-committed value, not the stale closure from
  // handleSubmit. `invalid` (wrong/weak) → wipe confirm only (preserve
  // primary for edit-and-retry). `conflict` (409 post-phase-flip) → wipe
  // primary too (the typed "new password" is not a valid login input).
  // `throttled`/`network`/`server` → touch nothing (user's input is fine,
  // the server isn't cooperating yet).
  useEffect(() => {
    if (!error) return;
    if (error.kind === "invalid") {
      setConfirm("");
    } else if (error.kind === "conflict") {
      setPassword("");
      setConfirm("");
    }
  }, [error]);

  const handleEnrollPasskey = useCallback(async () => {
    setPasskeyError(null);
    setPasskeyBusy("enrolling");
    try {
      const result = await passkey.enroll();
      if (!result.ok) {
        setPasskeyError(result.error);
        // Stay in the enroll step so the user can retry or skip; do NOT
        // auto-skip on error. Per the locked spec, the user can always
        // proceed without a passkey — but skipping unintentionally
        // would lose the failure signal.
        return;
      }
      // Success — drop the gate and let App.tsx re-render to the
      // authed dashboard (the token was already minted during
      // setPassword above, so phase is `"authed"` by the time
      // pendingEnroll flips false).
      setPendingEnroll(false);
    } finally {
      setPasskeyBusy(false);
    }
  }, [passkey]);

  const handleSkipEnroll = useCallback(() => {
    setPasskeyError(null);
    setPendingEnroll(false);
  }, []);

  const handleLoginWithPasskey = useCallback(async () => {
    setPasskeyError(null);
    // Clear the password-login error region too — the user is choosing
    // the passkey path, so a stale "wrong password" banner alongside
    // the passkey prompt is just noise.
    if (error) onClearError();
    setPasskeyBusy("logging-in");
    try {
      const result = await passkey.loginWithPasskey();
      if (!result.ok) {
        setPasskeyError(result.error);
        return;
      }
      // Success — usePasskey already stashed the token via setWebToken
      // and the auth-change event will re-render App.tsx into the
      // dashboard. Nothing more to do here.
    } finally {
      setPasskeyBusy(false);
    }
  }, [passkey, error, onClearError]);

  // ── Render: passkey-enroll sub-step ────────────────────────────────
  // Shown ONLY after a successful setPassword AND when the origin
  // supports passkey. Skippable. Inline errors per the locked taxonomy.
  if (pendingEnroll) {
    return (
      <div className="login-root">
        <div className="login-card">
          <h1 className="login-title">wisphive</h1>
          <p className="login-subtitle">Set up a passkey on this device?</p>
          <div className="login-setup">
            <p style={{ margin: 0 }}>
              Passkeys let you sign in with Touch ID, Windows Hello, or a
              security key instead of typing your password.
            </p>
            {passkeyError && (
              <div className="login-error login-error-invalid" role="alert">
                {passkeyErrorText(passkeyError)}
              </div>
            )}
            <button
              type="button"
              className="login-submit"
              onClick={() => void handleEnrollPasskey()}
              disabled={passkeyBusy !== false}
            >
              {passkeyBusy === "enrolling" ? "Enrolling passkey…" : "Enroll passkey"}
            </button>
            <button
              type="button"
              className="login-refresh"
              onClick={handleSkipEnroll}
              disabled={passkeyBusy !== false}
            >
              Skip for now
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="login-root">
      <div className="login-card">
        <h1 className="login-title">wisphive</h1>
        <p className="login-subtitle">
          {isSetup
            ? "Welcome. Set a password to finish setup."
            : "Sign in to review pending decisions."}
        </p>

        {/* Login-with-passkey button rendered ABOVE the password form so
            users who already have a passkey can skip the typing dance.
            Hidden on the `setup` phase (no credentials enrolled yet) and
            on origins that can't host the ceremony. The password form
            below stays as the always-available fallback. */}
        {!isSetup && showPasskeyAffordances && (
          <div className="login-passkey-cta">
            <button
              type="button"
              className="login-submit"
              onClick={() => void handleLoginWithPasskey()}
              disabled={passkeyBusy !== false || disabled}
            >
              {passkeyBusy === "logging-in" ? "Waiting for passkey…" : "Sign in with a passkey"}
            </button>
            {passkeyError && (
              <div className="login-error login-error-invalid" role="alert">
                {passkeyErrorText(passkeyError)}
              </div>
            )}
            <div className="login-divider" aria-hidden="true">
              <span>or use your password</span>
            </div>
          </div>
        )}

        <form onSubmit={handleSubmit} className="login-form">
          <label className="login-field">
            <span>{isSetup ? "New password" : "Password"}</span>
            <input
              // React re-uses the same DOM node across phase flips since
              // the JSX shape is identical — autoFocus only fires on
              // mount, so without a phase-keyed key the user is not
              // refocused after a 409 setup → login transition. Keying
              // by phase remounts the input cleanly.
              key={`password-${phase}`}
              type="password"
              autoComplete={isSetup ? "new-password" : "current-password"}
              autoFocus
              value={password}
              onChange={(e) => {
                setPassword(e.target.value);
                if (localError) setLocalError(null);
                if (error) onClearError();
              }}
              disabled={disabled}
              minLength={isSetup ? MIN_PASSWORD_LEN : undefined}
            />
          </label>
          {isSetup && (
            <label className="login-field">
              <span>Confirm password</span>
              <input
                type="password"
                autoComplete="new-password"
                value={confirm}
                onChange={(e) => {
                  setConfirm(e.target.value);
                  if (localError) setLocalError(null);
                }}
                disabled={disabled}
              />
            </label>
          )}
          <label className="login-field">
            <span>Device name</span>
            <input
              type="text"
              autoComplete="off"
              value={deviceName}
              onChange={(e) => setDeviceName(e.target.value)}
              disabled={disabled}
              placeholder="e.g. MacBook (Chrome)"
            />
          </label>
          {(localError || error) && (
            <div
              // When a localError is displayed, the styling should match
              // "invalid" — we're showing client-side validation text, not
              // echoing the server's error kind. Otherwise a stale
              // throttle-kind `error` would style the localError div with
              // the wrong color/border.
              className={`login-error login-error-${localError ? "invalid" : (error?.kind ?? "invalid")}`}
              role="alert"
            >
              {localError
                ? localError
                : error?.kind === "throttled" && countdown > 0
                  ? `Too many attempts — try again in ${countdown}s.`
                  : error?.message}
            </div>
          )}
          <button
            type="submit"
            className="login-submit"
            disabled={disabled || !password || (isSetup && !confirm)}
          >
            {submitting
              ? isSetup
                ? "Setting password…"
                : "Signing in…"
              : isSetup
                ? "Set password"
                : "Sign in"}
          </button>
          {isSetup && (
            <button
              type="button"
              className="login-refresh"
              onClick={() => void onRefreshStatus()}
              disabled={submitting}
            >
              I set it in a terminal — reload
            </button>
          )}
        </form>
      </div>
    </div>
  );
}
