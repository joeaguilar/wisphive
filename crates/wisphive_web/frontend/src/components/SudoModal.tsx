import { memo, useCallback, useEffect, useRef, useState } from "react";
import { apiFetch } from "../api";
import { Modal } from "./Modal";

export interface SudoModalError {
  kind: "invalid" | "throttled" | "network" | "server";
  message: string;
  retryAfter?: number;
}

interface Props {
  /** The sudo-class tool the user was trying to approve. Shown in copy so the
   * reauth prompt has visible context — "Bash" / "Edit" / etc. */
  toolName: string;
  /** Close the modal without retrying (user cancels or Esc). */
  onCancel: () => void;
  /** Server confirmed the reauth landed; caller should replay the queued
   * Approve. Wired in useWisphive → App. */
  onSuccess: () => void;
}

function isAbortError(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "name" in error &&
    error.name === "AbortError"
  );
}

/**
 * SudoModal — re-prompts for the account password when the daemon rejects a
 * sudo-class approve with `web_reauth_required`. Mirrors Login.tsx's error
 * shape and throttle countdown so the two auth surfaces feel identical.
 */
export const SudoModal = memo(function SudoModal({ toolName, onCancel, onSuccess }: Props) {
  const [password, setPassword] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<SudoModalError | null>(null);
  const [countdown, setCountdown] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const requestsRef = useRef<Set<AbortController>>(new Set());

  const fetchWithAbort = useCallback(
    async (path: string, init: RequestInit = {}): Promise<Response> => {
      const controller = new AbortController();
      requestsRef.current.add(controller);
      try {
        return await apiFetch(path, { ...init, signal: controller.signal });
      } finally {
        requestsRef.current.delete(controller);
      }
    },
    [],
  );

  // Requests issued by this modal only matter while it remains open.
  useEffect(() => {
    const requests = requestsRef.current;
    return () => {
      for (const controller of requests) {
        controller.abort();
      }
      requests.clear();
    };
  }, []);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Mirror Login.tsx's 429 countdown: trust the server-supplied Retry-After,
  // tick it down locally, clear the throttled error when it reaches zero.
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
          if (error?.kind === "throttled") setError(null);
          return 0;
        }
        return c - 1;
      });
    }, 1000);
    return () => clearInterval(interval);
  }, [error, isThrottled]);

  const disabled = submitting || countdown > 0;

  const handleSubmit = useCallback(
    async (event: React.FormEvent) => {
      event.preventDefault();
      if (disabled || !password) return;
      setSubmitting(true);
      setError(null);
      let aborted = false;
      try {
        const res = await fetchWithAbort("/api/auth/reauth", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ password }),
        });
        if (res.status === 200) {
          // Wipe the password from React state immediately (same belt-and-
          // braces reason as Login.tsx — shorten the memory-dump window).
          setPassword("");
          onSuccess();
          return;
        }
        if (res.status === 401) {
          setError({ kind: "invalid", message: "Invalid password." });
          return;
        }
        if (res.status === 429) {
          const retry = Number(res.headers.get("retry-after"));
          const retryAfter =
            Number.isFinite(retry) && retry > 0 ? Math.min(retry, 3600) : 30;
          setError({
            kind: "throttled",
            message: "Too many attempts.",
            retryAfter,
          });
          return;
        }
        if (res.status === 503) {
          setError({ kind: "server", message: "Daemon unreachable. Try again shortly." });
          return;
        }
        setError({ kind: "server", message: `Reauth failed (${res.status}).` });
      } catch (err) {
        if (isAbortError(err)) {
          aborted = true;
          return;
        }
        setError({
          kind: "network",
          message: `Could not reach daemon: ${err instanceof Error ? err.message : String(err)}`,
        });
      } finally {
        if (!aborted) setSubmitting(false);
      }
    },
    [disabled, password, onSuccess, fetchWithAbort],
  );

  return (
    <Modal title="Re-authenticate" onClose={onCancel}>
      <form onSubmit={handleSubmit} className="sudo-form">
        <p className="sudo-subtitle">
          Approve requires re-auth for <span className="sudo-tool">{toolName}</span>.
        </p>
        <label className="login-field">
          <span>Password</span>
          <input
            ref={inputRef}
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => {
              setPassword(e.target.value);
              if (error && error.kind !== "throttled") setError(null);
            }}
            disabled={disabled}
          />
        </label>
        {error && (
          <div className={`login-error login-error-${error.kind}`} role="alert">
            {error.kind === "throttled" && countdown > 0
              ? `Too many attempts — try again in ${countdown}s.`
              : error.message}
          </div>
        )}
        <div className="modal-actions">
          <button
            type="submit"
            className="login-submit"
            disabled={disabled || !password}
          >
            {submitting ? "Verifying…" : "Confirm"}
          </button>
          <button type="button" className="btn-cancel" onClick={onCancel} disabled={submitting}>
            Cancel
          </button>
        </div>
      </form>
    </Modal>
  );
});
