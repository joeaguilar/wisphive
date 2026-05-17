/**
 * usePasskey — frontend half of the WebAuthn ceremonies shipped by
 * itr#311 (PR-4 of #219). Wraps the four backend routes:
 *
 *   POST /api/auth/passkey/register/start  (bearer required)
 *   POST /api/auth/passkey/register/finish (bearer required)
 *   POST /api/auth/passkey/login/start     (unauthenticated)
 *   POST /api/auth/passkey/login/finish    (unauthenticated)
 *
 * around `navigator.credentials.create` / `navigator.credentials.get`
 * with the exact response shape #311 ships and the error taxonomy locked
 * in #312.
 *
 * ## Contract reminders from #311 review (do NOT drop these)
 *
 * 1. **Flattened response shape.** `/register/start` and `/login/start`
 *    return `{ session_id, publicKey: { ... } }`. The `session_id` is a
 *    top-level SIBLING of `publicKey`, not nested inside it. We destructure
 *    here so the browser-side call only ever sees `{ publicKey }` — the
 *    browser tolerates extra fields, but passing `session_id` into the
 *    WebAuthn struct would be a needless drift from the WebAuthn spec
 *    shape (and our lint might bite us later).
 *
 * 2. **`/login/start` consumes a throttle slot AND inserts a
 *    ChallengeStore row** (the row reaps on a 60s cadence). It must NEVER
 *    be called on page-mount — only on the user's explicit click of
 *    "Login with passkey". Calling on mount wastes rate-limit budget and
 *    grows the ChallengeStore without bound. The shape of this hook
 *    enforces that: `enroll` and `loginWithPasskey` are imperative
 *    functions, not auto-fired effects.
 *
 * 3. **Shared throttle with password login.** A run of failed passkey
 *    attempts locks out password login from the same IP and vice versa.
 *    Throttle banner copy is the caller's responsibility (Login.tsx);
 *    here we just surface the 429 as a `server_rejected` PasskeyError
 *    with the body text included.
 *
 * 4. **Counter regression returns 401 plain-text `"counter regression
 *    detected"`.** Either a cloned credential or a buggy authenticator.
 *    NEVER auto-retry — the user MUST see this clearly. We surface as
 *    `server_rejected` with the body message; Login.tsx renders verbatim.
 *
 * 5. **Error discriminant inconsistency** (until itr#320 normalises):
 *    - 400 origin-gate / 403 sudo-gate return JSON `{ error, message }`
 *    - other 4xx/5xx return plain text (`"unknown or expired session"`,
 *      `"invalid credential"`, `"counter regression detected"`).
 *    We branch on status code first, then attempt JSON parse only when
 *    the response carries an `application/json` Content-Type.
 *
 * 6. **Body cap is 32 KiB** on all four passkey routes. YubiKey
 *    attestation chains can approach this. Surfaces as a 413 in practice
 *    — we map to `server_rejected` and let the message text explain.
 *
 * ## base64url conversion
 *
 * Per the locked design: native `Uint8Array.fromBase64` / `toBase64`
 * with `{ alphabet: 'base64url' }` in the browser. Target browsers
 * (Chromium 134+, Firefox 137+) all support these — see #139 / the
 * /alignment session 2026-05-16.
 *
 * Vitest runs under jsdom on Node 22.12 (the harness baseline) which
 * does NOT yet ship these methods (Node 22.13+ / 23+). To keep tests
 * runnable without dragging in a polyfill, [`base64UrlEncode`] /
 * [`base64UrlDecode`] feature-detect the native methods and fall back
 * to a minimal `atob` / `btoa` shim. The fallback path is exercised
 * only in tests; production browsers take the fast native path.
 */

import { useMemo } from "react";
import { apiFetch, setWebToken } from "../api";

// ---------------------------------------------------------------------------
// Error taxonomy
// ---------------------------------------------------------------------------

/** PasskeyError discriminants — exact list locked in #312.
 *
 * Mapping:
 * - `unsupported`     — `navigator.credentials` absent OR caught `NotSupportedError`
 * - `cancelled`       — `NotAllowedError` (user cancelled OR dialog timeout)
 * - `origin_unavailable` — HTTP 400 with JSON body `error === "passkey_unavailable_on_this_origin"`
 * - `sudo_required`   — HTTP 403 with JSON body `error === "sudo_required_for_passkey_register"`
 *                       (Enterprise profile's pre-enroll re-auth gate; itr#313
 *                       wires the actual sudo IPC. Until then we surface a
 *                       friendly explanation instead of raw JSON.)
 * - `uv_failed`       — `ConstraintError` (UV requested but not met)
 * - `server_rejected` — any other 4xx/5xx. `message` is the response body verbatim.
 * - `network`         — `fetch` itself rejected (offline, CORS, etc.)
 * - `unknown`         — anything else. Full error logged to console; `message` is generic.
 */
export type PasskeyErrorKind =
  | "unsupported"
  | "cancelled"
  | "origin_unavailable"
  | "sudo_required"
  | "uv_failed"
  | "server_rejected"
  | "network"
  | "unknown";

export interface PasskeyError {
  kind: PasskeyErrorKind;
  message: string;
}

/** Result of a successful passkey login — the new bearer token to stash
 * into `setWebToken`. `enrolling_device_id` (if present) is the device
 * the credential was originally bound to; the SPA's "manage passkeys"
 * view (#220) needs it to look up the user's credentials. Not consumed
 * by Login.tsx in this PR but plumbed for caller use. */
export interface PasskeyLoginSuccess {
  ok: true;
  token: string;
  deviceId: string;
  enrollingDeviceId?: string;
}

export type PasskeyLoginResult = PasskeyLoginSuccess | { ok: false; error: PasskeyError };
export type PasskeyEnrollResult = { ok: true; credentialId: string } | { ok: false; error: PasskeyError };

// ---------------------------------------------------------------------------
// base64url helpers (native first, atob/btoa fallback for jsdom)
// ---------------------------------------------------------------------------

/** Type sliver for the staged ES2024 `Uint8Array.fromBase64` / `toBase64`
 * methods — TypeScript 5.9's lib.d.ts doesn't ship them yet (they're a
 * Stage-3 proposal at the time of writing). Declaring as `unknown`-typed
 * properties keeps the strict compiler happy without committing to a
 * speculative signature shape. */
interface NativeBase64Capable {
  fromBase64?: (s: string, opts?: { alphabet?: "base64" | "base64url" }) => Uint8Array;
}

interface NativeBase64Instance {
  toBase64?: (opts?: { alphabet?: "base64" | "base64url"; omitPadding?: boolean }) => string;
}

/** Encode a `Uint8Array` as a base64url string (no padding). Prefers
 * the native `Uint8Array.prototype.toBase64({ alphabet: 'base64url' })`
 * when available; falls back to a `btoa` shim for jsdom / older Node. */
export function base64UrlEncode(bytes: Uint8Array): string {
  const native = (bytes as Uint8Array & NativeBase64Instance).toBase64;
  if (typeof native === "function") {
    // Native path: omitPadding to match RFC 4648 §5 (URL-safe, unpadded).
    return native.call(bytes, { alphabet: "base64url", omitPadding: true });
  }
  // Fallback: build a binary string then btoa, then swap +/ for -_ and
  // strip padding. This is the pre-ES2024 idiom and is well-tested.
  let binary = "";
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  const b64 = btoa(binary);
  return b64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Encode an `ArrayBuffer` (the WebAuthn binary-slot type) as base64url.
 * Sugar for `base64UrlEncode(new Uint8Array(buffer))` — the wrap was
 * repeated 7× across the enroll/login finish-bodies. Extracted per the
 * WET principle (abstract after 3 repetitions). */
function bufferToB64u(buffer: ArrayBuffer): string {
  return base64UrlEncode(new Uint8Array(buffer));
}

/** Decode a base64url string (with or without padding) to a
 * `Uint8Array<ArrayBuffer>`. The explicit `<ArrayBuffer>` generic
 * (vs the default `<ArrayBufferLike>`) is what makes the result
 * assignable to WebAuthn's `BufferSource` slots — DOM lib types
 * reject `<SharedArrayBuffer>`-typed buffers from those positions.
 *
 * Prefers the native `Uint8Array.fromBase64({ alphabet: 'base64url' })`
 * when available; falls back to an `atob` shim for jsdom / older Node. */
export function base64UrlDecode(s: string): Uint8Array<ArrayBuffer> {
  const ctor = Uint8Array as unknown as NativeBase64Capable;
  if (typeof ctor.fromBase64 === "function") {
    // Cast: the staged TC39 method we stub-typed above doesn't pin
    // the buffer-kind generic; assume non-shared (the spec uses
    // ArrayBuffer in every browser implementation today).
    return ctor.fromBase64(s, { alphabet: "base64url" }) as Uint8Array<ArrayBuffer>;
  }
  // Fallback: normalize to base64 + padding, then atob to a binary string.
  const padded = s.replace(/-/g, "+").replace(/_/g, "/");
  const pad = padded.length % 4 === 0 ? "" : "=".repeat(4 - (padded.length % 4));
  const binary = atob(padded + pad);
  // Allocate the backing buffer explicitly as ArrayBuffer (not
  // ArrayBufferLike) so the returned view's generic narrows correctly.
  const buffer = new ArrayBuffer(binary.length);
  const out = new Uint8Array(buffer);
  for (let i = 0; i < binary.length; i++) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}

// ---------------------------------------------------------------------------
// Server response shapes (mirrors the Rust handlers in passkey.rs / lib.rs)
// ---------------------------------------------------------------------------

/** Subset of the WebAuthn `PublicKeyCredentialCreationOptions` JSON we
 * actually have to massage before handing to the browser. Other fields
 * (`rp`, `pubKeyCredParams`, `timeout`, `authenticatorSelection`, etc.)
 * are pass-through and don't need shape declarations here. */
interface CreationPublicKeyOptions {
  challenge: string;
  user: { id: string; name: string; displayName: string };
  excludeCredentials?: Array<{ id: string; type: string; transports?: string[] }>;
  [k: string]: unknown;
}

interface RegisterStartResponse {
  session_id: string;
  publicKey: CreationPublicKeyOptions;
  // Flattened response from #311 may carry other top-level pass-through
  // fields in future migrations — keep the rest under index signature.
  [k: string]: unknown;
}

interface RequestPublicKeyOptions {
  challenge: string;
  allowCredentials?: Array<{ id: string; type: string; transports?: string[] }>;
  [k: string]: unknown;
}

interface LoginStartResponse {
  session_id: string;
  publicKey: RequestPublicKeyOptions;
  [k: string]: unknown;
}

interface LoginFinishResponse {
  device_id: string;
  token: string;
  enrolling_device_id?: string;
}

interface RegisterFinishResponse {
  credential_id: string;
  created_at: string;
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/** Map a thrown `Error` from `navigator.credentials.create/get` (or
 * adjacent setup) to a `PasskeyError`. The WebAuthn API surfaces DOM
 * exceptions whose `name` is the stable discriminant — text messages
 * vary across browsers and locales, but `name` is normative. */
function classifyDomError(e: unknown): PasskeyError {
  if (e instanceof Error || (typeof e === "object" && e !== null && "name" in e)) {
    const name = (e as { name?: string }).name;
    const message = (e as { message?: string }).message ?? String(e);
    switch (name) {
      case "NotAllowedError":
        // User cancelled the prompt OR the browser timed it out. Both
        // present as the same DOMException name; we can't disambiguate
        // here so the message is intentionally neutral.
        return { kind: "cancelled", message: "Passkey prompt was cancelled or timed out." };
      case "NotSupportedError":
        // Browser doesn't implement WebAuthn (or doesn't support the
        // requested algorithm / authenticator type). Treat as
        // unsupported so Login.tsx can hide the affordance.
        return { kind: "unsupported", message: "This browser does not support passkeys." };
      case "ConstraintError":
        // UV (user verification) was required but the authenticator
        // couldn't satisfy it (e.g. a security key without biometric
        // capability under UV=required). Surface distinctly so the
        // operator knows to try a different authenticator.
        return { kind: "uv_failed", message: "User verification failed on the authenticator." };
      case "SecurityError":
      case "InvalidStateError":
      case "AbortError":
      case "UnknownError":
        // These map to "server_rejected"-style messages because the
        // browser is reporting a structural problem (wrong RP ID,
        // duplicate cred, ceremony aborted). We don't have a finer
        // discriminant for them in v1 — fall through to unknown so the
        // user sees the message verbatim and can report it.
        return { kind: "unknown", message: `${name}: ${message}` };
      default:
        // Catch-all for new / unhandled DOM exception names. Log the
        // full error so a future review can promote the new name to
        // a real discriminant.
        console.error("Unrecognised WebAuthn error:", e);
        return { kind: "unknown", message: message || "Unknown passkey error." };
    }
  }
  console.error("Non-Error thrown from WebAuthn call:", e);
  return { kind: "unknown", message: "Unknown passkey error." };
}

/** Map a non-OK `Response` to a `PasskeyError`. Branches on status code
 * first (cheapest), then peeks at Content-Type to decide whether to
 * try JSON parsing. The `origin_unavailable` discriminant is load-
 * bearing for Login.tsx's "hide enroll button" path, so we extract it
 * out of the JSON body when present. */
async function classifyHttpError(res: Response): Promise<PasskeyError> {
  const contentType = res.headers.get("content-type") ?? "";
  // Read the body once. Both branches (JSON / text) need it.
  const text = await res.text().catch(() => "");
  // JSON-shaped error responses come from two known routes today
  // (#311 review note 5):
  //   - 400 `passkey_unavailable_on_this_origin` (origin gate)
  //   - 403 `sudo_required_for_passkey_register` (Enterprise sudo gate)
  // Both ship `{ error, message }`. Branching JSON-parse on BOTH status
  // codes (not just 400) is what lets us pull out `sudo_required` —
  // before the fix this status was falling through to the plain-text
  // branch, where the user saw raw JSON rendered inline.
  if (
    (res.status === 400 || res.status === 403) &&
    contentType.includes("application/json")
  ) {
    try {
      const body = JSON.parse(text) as { error?: string; message?: string };
      if (res.status === 400 && body.error === "passkey_unavailable_on_this_origin") {
        return {
          kind: "origin_unavailable",
          message: body.message ?? "Passkey enrollment is not available on this origin.",
        };
      }
      if (res.status === 403 && body.error === "sudo_required_for_passkey_register") {
        // itr#313 will wire the actual sudo re-auth IPC. Until that
        // ships, the friendly message lives in Login.tsx::passkeyErrorText
        // (we keep this layer message-agnostic so the discriminant is
        // the single source of truth for rendering decisions).
        return {
          kind: "sudo_required",
          message:
            body.message ??
            "Passkey enrollment requires re-authentication (pending itr#313).",
        };
      }
      // Other JSON 4xx (e.g. malformed body) — surface the message
      // verbatim so the operator sees what the server complained about.
      return {
        kind: "server_rejected",
        message: body.message ?? body.error ?? text ?? `Server rejected request (${res.status}).`,
      };
    } catch {
      // Server claimed JSON but couldn't be parsed. Fall through to
      // plain-text handling — better to surface the raw body than
      // swallow it.
    }
  }
  // Plain-text branch covers the bulk of the routes' 4xx/5xx surface
  // (`"unknown or expired session"`, `"invalid credential"`,
  // `"counter regression detected"`, throttle 429, sudo-gate 403
  // when it returns text, etc.). Include the status code so silent
  // server bugs are obvious in the UI.
  return {
    kind: "server_rejected",
    message: text || `Server rejected request (${res.status}).`,
  };
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

export interface UsePasskey {
  /** Run the register ceremony. Caller is responsible for confirming
   * that `useAuthProfile().canEnrollPasskeyOnThisOrigin === true`
   * before invoking — calling on a non-supporting origin will return
   * `{ ok: false, error: { kind: 'origin_unavailable' } }`. */
  enroll: () => Promise<PasskeyEnrollResult>;
  /** Run the discoverable-credential login ceremony. On success the
   * new bearer is automatically stashed via `setWebToken` — the auth-
   * change event path will re-render the app shell. */
  loginWithPasskey: () => Promise<PasskeyLoginResult>;
}

export function usePasskey(): UsePasskey {
  // The hook is stateless — every call is one-shot. `useMemo` just
  // stabilises the returned object identity so consumers passing it
  // through `useCallback` deps don't churn.
  return useMemo<UsePasskey>(
    () => ({
      enroll: enrollImpl,
      loginWithPasskey: loginWithPasskeyImpl,
    }),
    [],
  );
}

/** Returns true when `navigator.credentials` is present AND its create
 * method is callable. Some embedded webviews (and jsdom) expose the
 * navigator object without the credentials property — check both. */
function hasWebAuthn(): boolean {
  return (
    typeof navigator !== "undefined" &&
    typeof navigator.credentials !== "undefined" &&
    typeof navigator.credentials.create === "function" &&
    typeof navigator.credentials.get === "function"
  );
}

async function enrollImpl(): Promise<PasskeyEnrollResult> {
  if (!hasWebAuthn()) {
    return {
      ok: false,
      error: { kind: "unsupported", message: "This browser does not support passkeys." },
    };
  }

  // (1) Server-side ceremony start.
  let startRes: Response;
  try {
    startRes = await apiFetch("/api/auth/passkey/register/start", { method: "POST" });
  } catch (e) {
    return {
      ok: false,
      error: {
        kind: "network",
        message: `Could not reach daemon: ${e instanceof Error ? e.message : String(e)}`,
      },
    };
  }
  if (!startRes.ok) {
    return { ok: false, error: await classifyHttpError(startRes) };
  }
  const startBody = (await startRes.json()) as RegisterStartResponse;
  // Defensive destructure: the backend's flattened shape (#311) places
  // `session_id` as a sibling of `publicKey`. We pull session_id aside
  // (rename to camelCase for local use; keep `session_id` on the wire
  // because the server expects snake_case) and hand ONLY `publicKey`
  // (mapped to BufferSources) to the browser. Passing extra fields
  // wouldn't fail the WebAuthn call but it'd be a needless drift from
  // the spec shape.
  const { session_id: sessionId, publicKey } = startBody;

  // (2) Convert base64url strings to ArrayBuffers/Uint8Arrays for the
  // browser API. Only the binary-typed fields need conversion; the
  // rest (rp, pubKeyCredParams, etc.) pass through untouched.
  //
  // S(sec)1 — the `base64UrlDecode` calls below throw `InvalidCharacter`
  // (native path) or a generic Error (fallback) on malformed base64url
  // input. A malicious or broken server could ship a non-base64url
  // string in `publicKey.challenge` or `publicKey.user.id`; we don't
  // want the throw to escape as an uncaught Promise rejection (no
  // PasskeyError taxonomy for the caller, console noise). Wrap the
  // whole construction in a single try so any decode failure becomes a
  // `server_rejected` with a clear message.
  let browserOpts: PublicKeyCredentialCreationOptions;
  try {
    browserOpts = {
      ...(publicKey as unknown as PublicKeyCredentialCreationOptions),
      challenge: base64UrlDecode(publicKey.challenge),
      user: {
        ...(publicKey.user as unknown as PublicKeyCredentialUserEntity),
        id: base64UrlDecode(publicKey.user.id),
      },
      ...(publicKey.excludeCredentials
        ? {
            excludeCredentials: publicKey.excludeCredentials.map((c) => ({
              id: base64UrlDecode(c.id),
              type: c.type as PublicKeyCredentialType,
              ...(c.transports ? { transports: c.transports as AuthenticatorTransport[] } : {}),
            })),
          }
        : {}),
    };
  } catch {
    return {
      ok: false,
      error: {
        kind: "server_rejected",
        message: "Invalid response from server (could not parse passkey options).",
      },
    };
  }

  // (3) Browser-side ceremony.
  let credential: Credential | null;
  try {
    credential = await navigator.credentials.create({ publicKey: browserOpts });
  } catch (e) {
    return { ok: false, error: classifyDomError(e) };
  }
  if (!credential) {
    // navigator.credentials.create returning null without throwing is
    // technically allowed by the spec but never observed in practice
    // on supported browsers. Treat as cancellation — the safe
    // user-visible message.
    return {
      ok: false,
      error: { kind: "cancelled", message: "Passkey prompt was cancelled." },
    };
  }

  // (4) Re-serialise the authenticator response back to the JSON shape
  // the Rust `RegisterPublicKeyCredential` deserialiser expects
  // (base64url strings for all binary fields).
  const pubKeyCred = credential as PublicKeyCredential;
  const att = pubKeyCred.response as AuthenticatorAttestationResponse;
  const transports =
    typeof att.getTransports === "function" ? att.getTransports() : undefined;
  const finishBody = {
    // Wire field stays snake_case (`session_id`) per the server's
    // RegisterFinishRequest shape — the rename is local-variable only.
    session_id: sessionId,
    credential: {
      id: pubKeyCred.id,
      rawId: bufferToB64u(pubKeyCred.rawId),
      type: pubKeyCred.type,
      response: {
        attestationObject: bufferToB64u(att.attestationObject),
        clientDataJSON: bufferToB64u(att.clientDataJSON),
        ...(transports && transports.length ? { transports } : {}),
      },
      // The Rust deserialiser accepts `clientExtensionResults` OR `extensions`
      // (see RegisterPublicKeyCredential's #[serde(alias = ...)]). We send
      // the spec-canonical `clientExtensionResults` so the wire stays clean
      // even if the alias is dropped later.
      clientExtensionResults: pubKeyCred.getClientExtensionResults(),
    },
  };

  let finishRes: Response;
  try {
    finishRes = await apiFetch("/api/auth/passkey/register/finish", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(finishBody),
    });
  } catch (e) {
    return {
      ok: false,
      error: {
        kind: "network",
        message: `Could not reach daemon: ${e instanceof Error ? e.message : String(e)}`,
      },
    };
  }
  if (!finishRes.ok) {
    return { ok: false, error: await classifyHttpError(finishRes) };
  }
  const finishBodyJson = (await finishRes.json()) as RegisterFinishResponse;
  return { ok: true, credentialId: finishBodyJson.credential_id };
}

async function loginWithPasskeyImpl(): Promise<PasskeyLoginResult> {
  if (!hasWebAuthn()) {
    return {
      ok: false,
      error: { kind: "unsupported", message: "This browser does not support passkeys." },
    };
  }

  // (1) Login start — note this consumes a throttle slot AND inserts a
  // ChallengeStore row. Caller MUST have triggered this from an
  // explicit user action; see module docstring.
  let startRes: Response;
  try {
    startRes = await apiFetch("/api/auth/passkey/login/start", { method: "POST" });
  } catch (e) {
    return {
      ok: false,
      error: {
        kind: "network",
        message: `Could not reach daemon: ${e instanceof Error ? e.message : String(e)}`,
      },
    };
  }
  if (!startRes.ok) {
    return { ok: false, error: await classifyHttpError(startRes) };
  }
  const startBody = (await startRes.json()) as LoginStartResponse;
  // M(cq)2 rename: local camelCase, wire stays snake_case.
  const { session_id: sessionId, publicKey } = startBody;

  // (2) Convert base64url → BufferSource for the browser. Discoverable-
  // credential login leaves `allowCredentials` empty (matching backend
  // `start_discoverable_authentication`); we still map any entries the
  // backend sends in future, just in case the route grows a
  // PasskeyAuthentication path later.
  //
  // S(sec)1 — same defensive wrap as the enroll path. A malformed
  // server response that fails to decode lands as `server_rejected`
  // rather than escaping the await as an uncaught throw.
  let browserOpts: PublicKeyCredentialRequestOptions;
  try {
    browserOpts = {
      ...(publicKey as unknown as PublicKeyCredentialRequestOptions),
      challenge: base64UrlDecode(publicKey.challenge),
      ...(publicKey.allowCredentials
        ? {
            allowCredentials: publicKey.allowCredentials.map((c) => ({
              id: base64UrlDecode(c.id),
              type: c.type as PublicKeyCredentialType,
              ...(c.transports ? { transports: c.transports as AuthenticatorTransport[] } : {}),
            })),
          }
        : {}),
    };
  } catch {
    return {
      ok: false,
      error: {
        kind: "server_rejected",
        message: "Invalid response from server (could not parse passkey options).",
      },
    };
  }

  // (3) Browser-side ceremony.
  let credential: Credential | null;
  try {
    credential = await navigator.credentials.get({ publicKey: browserOpts });
  } catch (e) {
    return { ok: false, error: classifyDomError(e) };
  }
  if (!credential) {
    return {
      ok: false,
      error: { kind: "cancelled", message: "Passkey prompt was cancelled." },
    };
  }

  // (4) Re-serialise to the wire shape the Rust `PublicKeyCredential`
  // deserialiser expects (base64url strings).
  const pubKeyCred = credential as PublicKeyCredential;
  const asr = pubKeyCred.response as AuthenticatorAssertionResponse;
  const finishBody = {
    // Wire stays snake_case (`session_id`) per the server's
    // LoginFinishRequest shape — the rename is local-variable only.
    session_id: sessionId,
    credential: {
      id: pubKeyCred.id,
      rawId: bufferToB64u(pubKeyCred.rawId),
      type: pubKeyCred.type,
      response: {
        authenticatorData: bufferToB64u(asr.authenticatorData),
        clientDataJSON: bufferToB64u(asr.clientDataJSON),
        signature: bufferToB64u(asr.signature),
        // `userHandle` is `ArrayBuffer | null` per the spec; the Rust
        // deserialiser treats absence as None, so we omit when null.
        ...(asr.userHandle ? { userHandle: bufferToB64u(asr.userHandle) } : {}),
      },
      clientExtensionResults: pubKeyCred.getClientExtensionResults(),
    },
  };

  let finishRes: Response;
  try {
    finishRes = await apiFetch("/api/auth/passkey/login/finish", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(finishBody),
    });
  } catch (e) {
    return {
      ok: false,
      error: {
        kind: "network",
        message: `Could not reach daemon: ${e instanceof Error ? e.message : String(e)}`,
      },
    };
  }
  if (!finishRes.ok) {
    return { ok: false, error: await classifyHttpError(finishRes) };
  }
  const finishBodyJson = (await finishRes.json()) as LoginFinishResponse;
  // S(sec)2: validate the token shape BEFORE writing it to localStorage
  // via setWebToken. A malformed / truncated finish response that we
  // happily stashed would leave the SPA looking authed (local token
  // present) but every subsequent API call would 401 — the user lands
  // in a confusing logout loop. Better to surface "try again" up front.
  if (typeof finishBodyJson.token !== "string" || finishBodyJson.token.length === 0) {
    return {
      ok: false,
      error: {
        kind: "server_rejected",
        message: "Login succeeded but token was missing — please try again.",
      },
    };
  }
  // Stash the new bearer through the shared auth-change event path so
  // any subscriber (useAuth) re-gates the app to authed without the
  // caller wiring anything.
  setWebToken(finishBodyJson.token);
  return {
    ok: true,
    token: finishBodyJson.token,
    deviceId: finishBodyJson.device_id,
    ...(finishBodyJson.enrolling_device_id
      ? { enrollingDeviceId: finishBodyJson.enrolling_device_id }
      : {}),
  };
}
