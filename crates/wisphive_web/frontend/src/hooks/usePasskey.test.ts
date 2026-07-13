/**
 * usePasskey — Vitest coverage of the enroll + login ceremonies plus
 * the full error taxonomy locked in #312.
 *
 * The hook is a black box from the caller's perspective: feed in a
 * mocked navigator.credentials + a mocked fetch, assert the right
 * POST sequence + result discriminant. We don't pin internal
 * implementation details (effect identity, useMemo cache shape) so
 * a future refactor that keeps the contract green doesn't break tests.
 *
 * Mocking strategy:
 *   - `vi.stubGlobal('fetch', vi.fn())` replaces apiFetch's underlying
 *     fetch (apiFetch wraps but calls global fetch). We assert on path
 *     + body to catch routing typos AND base64url round-trip drift.
 *   - `vi.stubGlobal('navigator', { credentials: { create, get } })`
 *     replaces the WebAuthn surface. jsdom doesn't ship navigator.credentials
 *     so this is the only way to exercise the ceremony.
 *   - base64url helpers are tested implicitly via the round-trip: we
 *     send a known challenge string, assert the browser saw the matching
 *     bytes, and assert the finish POST sends back base64url that round-
 *     trips to the same bytes.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";

import {
  base64UrlDecode,
  base64UrlEncode,
  usePasskey,
} from "./usePasskey";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

function jsonResponse(body: unknown, init?: { status?: number }): Response {
  return new Response(JSON.stringify(body), {
    status: init?.status ?? 200,
    headers: { "content-type": "application/json" },
  });
}

function textResponse(text: string, status: number, contentType = "text/plain"): Response {
  return new Response(text, { status, headers: { "content-type": contentType } });
}

/** Build a fake WebAuthn `PublicKeyCredential` whose `response.attestationObject`
 * + `response.clientDataJSON` are real ArrayBuffers, so the encode-round-trip
 * exercised below produces deterministic base64url strings the assertions
 * can pin against. */
function fakeRegistrationCredential(): PublicKeyCredential {
  const attestation = new Uint8Array([0xde, 0xad, 0xbe, 0xef]).buffer;
  const clientData = new TextEncoder().encode('{"type":"webauthn.create"}').buffer;
  const rawId = new Uint8Array([1, 2, 3, 4]).buffer;
  return {
    id: "credential-id-1",
    rawId,
    type: "public-key",
    authenticatorAttachment: null,
    getClientExtensionResults: () => ({}),
    response: {
      attestationObject: attestation,
      clientDataJSON: clientData,
      getTransports: () => ["usb"],
      // Newer methods left unimplemented — the hook doesn't touch them
      // on the register path.
    } as unknown as AuthenticatorAttestationResponse,
  } as unknown as PublicKeyCredential;
}

/** Same idea for the login (assertion) ceremony. */
function fakeAssertionCredential(): PublicKeyCredential {
  const authData = new Uint8Array([0xaa, 0xbb, 0xcc]).buffer;
  const clientData = new TextEncoder().encode('{"type":"webauthn.get"}').buffer;
  const signature = new Uint8Array([0x99, 0x88, 0x77, 0x66]).buffer;
  return {
    id: "credential-id-1",
    rawId: new Uint8Array([1, 2, 3, 4]).buffer,
    type: "public-key",
    authenticatorAttachment: null,
    getClientExtensionResults: () => ({}),
    response: {
      authenticatorData: authData,
      clientDataJSON: clientData,
      signature,
      userHandle: null,
    } as unknown as AuthenticatorAssertionResponse,
  } as unknown as PublicKeyCredential;
}

interface CredentialsMock {
  create: ReturnType<typeof vi.fn>;
  get: ReturnType<typeof vi.fn>;
}

function installNavigatorCredentials(): CredentialsMock {
  const credentials: CredentialsMock = { create: vi.fn(), get: vi.fn() };
  vi.stubGlobal("navigator", {
    credentials,
    // userAgent is read by Login.tsx's defaultDeviceName helper; not
    // strictly needed for usePasskey tests, but harmless to provide a
    // value so any incidental access doesn't throw.
    userAgent: "TestUA/1.0",
  });
  return credentials;
}

function removeNavigatorCredentials(): void {
  vi.stubGlobal("navigator", { userAgent: "TestUA/1.0" });
}

// ---------------------------------------------------------------------------
// base64url helper round-trip
// ---------------------------------------------------------------------------

describe("base64url helpers", () => {
  it("round-trips a known byte sequence", () => {
    // RFC 4648 §5 example: input 0x14fb9c03d97e → "FPucA9l-"
    const bytes = new Uint8Array([0x14, 0xfb, 0x9c, 0x03, 0xd9, 0x7e]);
    const encoded = base64UrlEncode(bytes);
    expect(encoded).toBe("FPucA9l-");
    const decoded = base64UrlDecode(encoded);
    expect(Array.from(decoded)).toEqual(Array.from(bytes));
  });

  it("decodes a string with URL-safe characters (no padding)", () => {
    const decoded = base64UrlDecode("FPucA9l-");
    expect(decoded.length).toBe(6);
    expect(decoded[5]).toBe(0x7e);
  });

  it("decodes a padded base64url string too (browsers send unpadded; be liberal)", () => {
    // base64url with padding should still decode — defensive against
    // future server changes or clients that re-add padding.
    const decoded = base64UrlDecode("FPucA9k=");
    expect(decoded.length).toBe(5);
  });
});

// ---------------------------------------------------------------------------
// usePasskey.enroll
// ---------------------------------------------------------------------------

describe("usePasskey.enroll", () => {
  let credentials: CredentialsMock;
  beforeEach(() => {
    credentials = installNavigatorCredentials();
    try {
      localStorage.removeItem("wisphive-web-token");
    } catch {
      /* jsdom */
    }
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("happy path: POST start, browser create, POST finish, round-trips base64url", async () => {
    // The challenge the server hands us — assert the browser sees the
    // matching binary form (after base64url decode).
    const challengeB64 = base64UrlEncode(new Uint8Array([0x10, 0x20, 0x30]));
    const userIdB64 = base64UrlEncode(new Uint8Array([0x42, 0x43]));
    const fetchMock = vi
      .fn()
      // /register/start
      .mockResolvedValueOnce(
        jsonResponse({
          session_id: "sess-abc",
          publicKey: {
            challenge: challengeB64,
            rp: { id: "localhost", name: "Wisphive" },
            user: { id: userIdB64, name: "u", displayName: "U" },
            pubKeyCredParams: [{ type: "public-key", alg: -7 }],
            timeout: 60000,
          },
        }),
      )
      // /register/finish
      .mockResolvedValueOnce(
        jsonResponse({ credential_id: "cred-id-1", created_at: "2026-05-17T00:00:00Z" }),
      );
    vi.stubGlobal("fetch", fetchMock);

    credentials.create.mockResolvedValue(fakeRegistrationCredential());

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.credentialId).toBe("cred-id-1");
    }

    // Assert the POST sequence.
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(fetchMock.mock.calls[0][0]).toBe("/api/auth/passkey/register/start");
    expect(fetchMock.mock.calls[1][0]).toBe("/api/auth/passkey/register/finish");

    // Browser call: challenge MUST be the decoded bytes, NOT the
    // base64url string. user.id likewise. And session_id MUST NOT
    // appear inside the publicKey blob (the backend ships it as a
    // flattened sibling; passing it to navigator would be a needless
    // drift from the WebAuthn spec).
    const browserArg = credentials.create.mock.calls[0][0] as CredentialCreationOptions;
    const pk = browserArg.publicKey!;
    expect(pk.challenge).toBeInstanceOf(Uint8Array);
    expect(Array.from(pk.challenge as Uint8Array)).toEqual([0x10, 0x20, 0x30]);
    expect(pk.user.id).toBeInstanceOf(Uint8Array);
    expect(Array.from(pk.user.id as Uint8Array)).toEqual([0x42, 0x43]);
    expect((pk as unknown as Record<string, unknown>).session_id).toBeUndefined();

    // Finish POST body: session_id at top level, base64url-encoded
    // credential fields. Round-trip the rawId to assert encoding.
    const finishInit = fetchMock.mock.calls[1][1] as RequestInit;
    const finishBody = JSON.parse(finishInit.body as string);
    expect(finishBody.session_id).toBe("sess-abc");
    expect(finishBody.credential.id).toBe("credential-id-1");
    expect(finishBody.credential.type).toBe("public-key");
    expect(Array.from(base64UrlDecode(finishBody.credential.rawId))).toEqual([1, 2, 3, 4]);
    // attestationObject round-trips to the deadbeef bytes we put in
    // fakeRegistrationCredential — proves the encode path is binary-clean.
    expect(Array.from(base64UrlDecode(finishBody.credential.response.attestationObject))).toEqual([
      0xde, 0xad, 0xbe, 0xef,
    ]);
    // Transports passed through from getTransports().
    expect(finishBody.credential.response.transports).toEqual(["usb"]);
  });

  it("maps navigator.credentials missing → unsupported", async () => {
    removeNavigatorCredentials();
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("unsupported");
    }
    // Did not call /start — the unsupported check must short-circuit
    // before consuming a throttle slot on the server.
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("maps NotSupportedError from create → unsupported", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse({
        session_id: "s",
        publicKey: {
          challenge: base64UrlEncode(new Uint8Array([1])),
          rp: { id: "localhost", name: "W" },
          user: { id: base64UrlEncode(new Uint8Array([2])), name: "u", displayName: "U" },
          pubKeyCredParams: [],
        },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const err = new Error("not supported");
    err.name = "NotSupportedError";
    credentials.create.mockRejectedValue(err);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.kind).toBe("unsupported");
  });

  it("maps NotAllowedError → cancelled", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse({
        session_id: "s",
        publicKey: {
          challenge: base64UrlEncode(new Uint8Array([1])),
          rp: { id: "localhost", name: "W" },
          user: { id: base64UrlEncode(new Uint8Array([2])), name: "u", displayName: "U" },
          pubKeyCredParams: [],
        },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const err = new Error("user cancelled");
    err.name = "NotAllowedError";
    credentials.create.mockRejectedValue(err);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.kind).toBe("cancelled");
  });

  it("maps ConstraintError → uv_failed", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse({
        session_id: "s",
        publicKey: {
          challenge: base64UrlEncode(new Uint8Array([1])),
          rp: { id: "localhost", name: "W" },
          user: { id: base64UrlEncode(new Uint8Array([2])), name: "u", displayName: "U" },
          pubKeyCredParams: [],
        },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const err = new Error("constraint");
    err.name = "ConstraintError";
    credentials.create.mockRejectedValue(err);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.kind).toBe("uv_failed");
  });

  it("maps HTTP 400 with passkey_unavailable_on_this_origin JSON → origin_unavailable", async () => {
    // The backend's `passkey_unavailable_response` shape — assert we
    // parse the discriminant out of the JSON body, NOT pattern-match
    // the human message.
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse(
        {
          error: "passkey_unavailable_on_this_origin",
          message: "Passkey enrollment is not available on this origin under the active profile.",
        },
        { status: 400 },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("origin_unavailable");
      expect(r.error.message).toContain("not available on this origin");
    }
  });

  // M2 fix: 403 JSON `sudo_required_for_passkey_register` MUST be
  // pulled out as its own discriminant, NOT fall through to
  // `server_rejected` (which is how the user used to see raw JSON in
  // the inline error region — Enterprise's sudo gate has no recovery
  // path until itr#313 wires the IPC).
  it("maps HTTP 403 with sudo_required_for_passkey_register JSON → sudo_required", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse(
        {
          error: "sudo_required_for_passkey_register",
          message: "Passkey enrollment requires re-authentication.",
        },
        { status: 403 },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("sudo_required");
      // Hook surfaces the server's message verbatim; the
      // user-facing wording is owned by Login.tsx::passkeyErrorText
      // (tested separately).
      expect(r.error.message).toBe("Passkey enrollment requires re-authentication.");
    }
  });

  // Regression: a 403 that is NOT the sudo discriminant MUST still
  // fall through to `server_rejected` (don't accidentally turn every
  // 403 into `sudo_required`).
  it("maps unrelated 403 JSON → server_rejected (not sudo_required)", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse(
        { error: "some_other_403_reason", message: "not the sudo gate" },
        { status: 403 },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("server_rejected");
    }
  });

  it("maps HTTP 500 (plain text) → server_rejected with body text", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(textResponse("internal error", 500));
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("server_rejected");
      expect(r.error.message).toBe("internal error");
    }
  });

  it("maps fetch reject (network down) → network", async () => {
    const fetchMock = vi.fn().mockRejectedValueOnce(new TypeError("Failed to fetch"));
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("network");
      expect(r.error.message).toContain("Failed to fetch");
    }
  });

  it("maps a non-DOMException throw from create → unknown", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      jsonResponse({
        session_id: "s",
        publicKey: {
          challenge: base64UrlEncode(new Uint8Array([1])),
          rp: { id: "localhost", name: "W" },
          user: { id: base64UrlEncode(new Uint8Array([2])), name: "u", displayName: "U" },
          pubKeyCredParams: [],
        },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    // Silence the console.error the hook emits for unrecognised errors —
    // the test deliberately triggers that path and we don't want it
    // bleeding into Vitest's report.
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    credentials.create.mockRejectedValue("a string, not an Error");

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.enroll();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.kind).toBe("unknown");
    errorSpy.mockRestore();
  });
});

// ---------------------------------------------------------------------------
// usePasskey.loginWithPasskey
// ---------------------------------------------------------------------------

describe("usePasskey.loginWithPasskey", () => {
  let credentials: CredentialsMock;
  beforeEach(() => {
    credentials = installNavigatorCredentials();
    try {
      localStorage.removeItem("wisphive-web-token");
    } catch {
      /* jsdom */
    }
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("happy path: POST start, browser get, POST finish, returns token + enrolling_device_id, stashes token", async () => {
    const challengeB64 = base64UrlEncode(new Uint8Array([0x77, 0x88, 0x99]));
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({
          session_id: "login-sess-1",
          publicKey: {
            challenge: challengeB64,
            rpId: "localhost",
            allowCredentials: [],
            userVerification: "preferred",
            timeout: 60000,
          },
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          device_id: "device-fresh",
          token: "raw-bearer-token",
          enrolling_device_id: "device-enrolled-original",
        }),
      );
    vi.stubGlobal("fetch", fetchMock);
    credentials.get.mockResolvedValue(fakeAssertionCredential());

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.loginWithPasskey();
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.token).toBe("raw-bearer-token");
      expect(r.deviceId).toBe("device-fresh");
      expect(r.enrollingDeviceId).toBe("device-enrolled-original");
    }

    // POST sequence: /login/start then /login/finish.
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(fetchMock.mock.calls[0][0]).toBe("/api/auth/passkey/login/start");
    expect(fetchMock.mock.calls[1][0]).toBe("/api/auth/passkey/login/finish");

    // Browser call: challenge is the decoded bytes; session_id stripped.
    const browserArg = credentials.get.mock.calls[0][0] as CredentialRequestOptions;
    const pk = browserArg.publicKey!;
    expect(Array.from(pk.challenge as Uint8Array)).toEqual([0x77, 0x88, 0x99]);
    expect((pk as unknown as Record<string, unknown>).session_id).toBeUndefined();

    // Token MUST be stashed into localStorage so the app shell re-gates
    // to authed via the subscribeAuthChange path.
    expect(localStorage.getItem("wisphive-web-token")).toBe("raw-bearer-token");
  });

  it("counter-regression 401 plain-text body → server_rejected with verbatim message", async () => {
    // Backend returns `"counter regression detected"` as plain text.
    // Frontend MUST surface the message intact — auto-retry is
    // forbidden per #311 review (could be a cloned credential).
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({
          session_id: "s",
          publicKey: {
            challenge: base64UrlEncode(new Uint8Array([1])),
            rpId: "localhost",
            allowCredentials: [],
            userVerification: "preferred",
          },
        }),
      )
      .mockResolvedValueOnce(textResponse("counter regression detected", 401));
    vi.stubGlobal("fetch", fetchMock);
    credentials.get.mockResolvedValue(fakeAssertionCredential());

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.loginWithPasskey();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("server_rejected");
      expect(r.error.message).toBe("counter regression detected");
    }
    // Token NOT stashed on failure.
    expect(localStorage.getItem("wisphive-web-token")).toBeNull();
  });

  it("throttle 429 plain-text body → server_rejected", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(textResponse("throttled", 429));
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.loginWithPasskey();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("server_rejected");
      expect(r.error.message).toBe("throttled");
    }
    // Browser was NOT invoked — throttle short-circuit at /start
    // means we never proceed to the WebAuthn prompt.
    expect(credentials.get).not.toHaveBeenCalled();
  });

  it("returns enrollingDeviceId undefined when backend omits it", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({
          session_id: "s",
          publicKey: {
            challenge: base64UrlEncode(new Uint8Array([1])),
            rpId: "localhost",
            allowCredentials: [],
            userVerification: "preferred",
          },
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({ device_id: "d", token: "t" }),
      );
    vi.stubGlobal("fetch", fetchMock);
    credentials.get.mockResolvedValue(fakeAssertionCredential());

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.loginWithPasskey();
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.enrollingDeviceId).toBeUndefined();
    }
  });

  // S(cq)6: pin the conditional-spread on `userHandle`. Until this test
  // existed every loginWithPasskey path used a fake credential with
  // `userHandle: null`, so the encoded-userHandle branch (and its
  // base64url encoding) was unexercised. A future refactor that
  // broke the encoding here would have slipped through.
  it("encodes userHandle to base64url when the authenticator returns one", async () => {
    const userHandleBytes = new Uint8Array([7, 8, 9]);
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({
          session_id: "s",
          publicKey: {
            challenge: base64UrlEncode(new Uint8Array([1])),
            rpId: "localhost",
            allowCredentials: [],
            userVerification: "preferred",
          },
        }),
      )
      .mockResolvedValueOnce(jsonResponse({ device_id: "d", token: "t" }));
    vi.stubGlobal("fetch", fetchMock);
    // Build a credential with a real userHandle ArrayBuffer (not null).
    const authData = new Uint8Array([0xaa, 0xbb, 0xcc]).buffer;
    const clientData = new TextEncoder().encode('{"type":"webauthn.get"}').buffer;
    const signature = new Uint8Array([0x99, 0x88, 0x77, 0x66]).buffer;
    const credWithHandle = {
      id: "credential-id-1",
      rawId: new Uint8Array([1, 2, 3, 4]).buffer,
      type: "public-key",
      authenticatorAttachment: null,
      getClientExtensionResults: () => ({}),
      response: {
        authenticatorData: authData,
        clientDataJSON: clientData,
        signature,
        userHandle: userHandleBytes.buffer,
      } as unknown as AuthenticatorAssertionResponse,
    } as unknown as PublicKeyCredential;
    credentials.get.mockResolvedValue(credWithHandle);

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.loginWithPasskey();
    expect(r.ok).toBe(true);

    // Pull the finish POST body and assert userHandle round-trips back
    // to the original bytes (proves base64url encoding ran on the
    // non-null branch).
    const finishInit = fetchMock.mock.calls[1][1] as RequestInit;
    const finishBody = JSON.parse(finishInit.body as string);
    expect(typeof finishBody.credential.response.userHandle).toBe("string");
    expect(Array.from(base64UrlDecode(finishBody.credential.response.userHandle))).toEqual([
      7, 8, 9,
    ]);
  });

  // S(sec)2: a finish response that lacks a token (or ships an empty
  // string) MUST fail before reaching setWebToken — otherwise the SPA
  // appears authed (local state thinks we have a token) while every
  // API call 401s, trapping the user in a logout loop.
  it("rejects a finish response with a missing token (does NOT stash it)", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({
          session_id: "s",
          publicKey: {
            challenge: base64UrlEncode(new Uint8Array([1])),
            rpId: "localhost",
            allowCredentials: [],
            userVerification: "preferred",
          },
        }),
      )
      .mockResolvedValueOnce(jsonResponse({ device_id: "d", token: "" }));
    vi.stubGlobal("fetch", fetchMock);
    credentials.get.mockResolvedValue(fakeAssertionCredential());

    const { result } = renderHook(() => usePasskey());
    const r = await result.current.loginWithPasskey();
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("server_rejected");
      expect(r.error.message).toMatch(/token was missing/i);
    }
    // Token MUST NOT be stashed.
    expect(localStorage.getItem("wisphive-web-token")).toBeNull();
  });

  it("aborts an in-flight fetch when the consumer unmounts", async () => {
    let signal: AbortSignal | undefined;
    const fetchMock = vi.fn(
      (_path: string, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          signal = init?.signal ?? undefined;
          signal?.addEventListener("abort", () => {
            reject(new DOMException("The operation was aborted.", "AbortError"));
          });
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result, unmount } = renderHook(() => usePasskey());
    const login = result.current.loginWithPasskey();
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    unmount();
    expect(signal?.aborted).toBe(true);

    await expect(login).resolves.toMatchObject({
      ok: false,
      error: { kind: "network" },
    });
  });
});
