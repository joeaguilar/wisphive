import { useCallback, useEffect, useState } from "react";
import type { AuthError, UseAuth } from "../hooks/useAuth";

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
  const isSetup = phase === "setup";

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
        }
        // Failure branches wipe defensively based on the outcome: the
        // updated `error` is committed by the time the next render runs,
        // but we read what's most recently-set via the effect below
        // rather than trying to read the post-setState value here.
      } finally {
        setSubmitting(false);
      }
    },
    [disabled, isSetup, password, confirm, deviceName, onLogin, onSetPassword],
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

  return (
    <div className="login-root">
      <div className="login-card">
        <h1 className="login-title">wisphive</h1>
        <p className="login-subtitle">
          {isSetup
            ? "Welcome. Set a password to finish setup."
            : "Sign in to review pending decisions."}
        </p>

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
