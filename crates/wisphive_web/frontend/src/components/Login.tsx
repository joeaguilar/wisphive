import { useCallback, useEffect, useState } from "react";
import type { AuthError, UseAuth } from "../hooks/useAuth";

interface Props {
  phase: UseAuth["phase"];
  error: AuthError | null;
  onLogin: UseAuth["login"];
  onClearError: UseAuth["clearError"];
  onRefreshStatus: UseAuth["refreshStatus"];
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
  onClearError,
  onRefreshStatus,
}: Props) {
  const [password, setPassword] = useState("");
  const [deviceName, setDeviceName] = useState(defaultDeviceName);
  const [submitting, setSubmitting] = useState(false);
  const [countdown, setCountdown] = useState(0);

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

  const disabled = submitting || countdown > 0 || phase === "setup";

  const onSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (disabled || !password) return;
      setSubmitting(true);
      try {
        const ok = await onLogin(password, deviceName);
        // On success, wipe the password from React state immediately —
        // React GC eventually reclaims closure-captured strings, but
        // shortening the window is cheap belt-and-braces against memory
        // dumps and devtools inspection. On failure, keep the field
        // populated so the user can correct a typo.
        if (ok) setPassword("");
      } finally {
        setSubmitting(false);
      }
    },
    [disabled, password, deviceName, onLogin],
  );

  return (
    <div className="login-root">
      <div className="login-card">
        <h1 className="login-title">wisphive</h1>
        <p className="login-subtitle">
          {phase === "setup"
            ? "No password is set on this host yet."
            : "Sign in to review pending decisions."}
        </p>

        {phase === "setup" ? (
          <div className="login-setup">
            <p>
              Run this on the host, then return here:
            </p>
            <pre className="login-code">wisphive web set-password</pre>
            <button
              type="button"
              className="login-refresh"
              onClick={() => void onRefreshStatus()}
            >
              I've set the password
            </button>
          </div>
        ) : (
          <form onSubmit={onSubmit} className="login-form">
            <label className="login-field">
              <span>Password</span>
              <input
                type="password"
                autoComplete="current-password"
                autoFocus
                value={password}
                onChange={(e) => {
                  setPassword(e.target.value);
                  if (error) onClearError();
                }}
                disabled={disabled}
              />
            </label>
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
            {error && (
              <div className={`login-error login-error-${error.kind}`} role="alert">
                {error.kind === "throttled" && countdown > 0
                  ? `Too many attempts — try again in ${countdown}s.`
                  : error.message}
              </div>
            )}
            <button
              type="submit"
              className="login-submit"
              disabled={disabled || !password}
            >
              {submitting ? "Signing in…" : "Sign in"}
            </button>
          </form>
        )}
      </div>
    </div>
  );
}
