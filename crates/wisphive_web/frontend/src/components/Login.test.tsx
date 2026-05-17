/**
 * Login.tsx — Vitest coverage of the passkey integration (#312) + the
 * `authed-pending-enroll` gate wired in the itr#312 review pass.
 *
 * The component is driven by two hooks (`useAuthProfile`, `usePasskey`)
 * + a useAuth callback bundle. We mock the two passkey-adjacent hooks
 * at the module boundary so we can independently flip
 * `canEnrollPasskeyOnThisOrigin` and the enroll/login outcomes without
 * standing up a full fetch fake. The pre-existing password-form
 * behavior (validated by manual smoke since itr#217) is exercised
 * indirectly via the "passkey UI does/doesn't hide the password form"
 * tests — the form keeps working as the always-available fallback
 * regardless of profile state.
 *
 * What this file pins:
 *   - Login-with-passkey button shown only when canEnrollPasskeyOnThisOrigin=true
 *     AND phase=unauthed (never on setup phase — no creds enrolled yet)
 *   - Login-with-passkey button hidden when canEnrollPasskeyOnThisOrigin=false
 *   - Failed setPassword does NOT enter the enroll step.
 *   - canEnrollPasskeyOnThisOrigin=false → setPassword bypasses the
 *     enroll step entirely (returns straight to whatever the parent
 *     renders for authed).
 *   - Passkey errors render the per-kind inline message (including
 *     the new `sudo_required` discriminant added in the M2 fix).
 *   - **M1 regression**: when the component is mounted under a
 *     real-useAuth harness (the harness this file ships), a successful
 *     setPassword leaves the enroll card mounted instead of unmounting
 *     Login before the local pendingEnroll flag could render. This is
 *     the exact bug the original prop-mock test missed.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Mock the two hooks so we can drive their outputs deterministically.
// `vi.mock` is hoisted; the factories below define the *default* return
// shape and individual tests override via `vi.mocked(...).mockReturnValue(...)`.
vi.mock("../hooks/useAuthProfile", () => ({
  useAuthProfile: vi.fn(() => ({
    profile: "local-lan",
    canEnrollPasskeyOnThisOrigin: true,
    passkeyRequired: false,
    allowEphemeralListener: true,
    loaded: true,
  })),
}));

vi.mock("../hooks/usePasskey", () => ({
  usePasskey: vi.fn(() => ({
    enroll: vi.fn().mockResolvedValue({ ok: true, credentialId: "cred-1" }),
    loginWithPasskey: vi
      .fn()
      .mockResolvedValue({ ok: true, token: "t", deviceId: "d" }),
  })),
}));

import { Login } from "./Login";
import { useAuth } from "../hooks/useAuth";
import { useAuthProfile } from "../hooks/useAuthProfile";
import { usePasskey } from "../hooks/usePasskey";

const mockedUseAuthProfile = vi.mocked(useAuthProfile);
const mockedUsePasskey = vi.mocked(usePasskey);

/** Build a Props bundle with sensible defaults — individual tests
 * override the bits they care about. Keeps each `render` call small
 * and intention-revealing. The default `onCompleteEnrollGate` is a
 * `vi.fn()` so tests that don't drive a real useAuth can still assert
 * it was called. */
function renderLogin(overrides: Partial<React.ComponentProps<typeof Login>> = {}) {
  const defaults: React.ComponentProps<typeof Login> = {
    phase: "unauthed",
    error: null,
    onLogin: vi.fn().mockResolvedValue(true),
    onSetPassword: vi.fn().mockResolvedValue(true),
    onCompleteEnrollGate: vi.fn(),
    onClearError: vi.fn(),
    onRefreshStatus: vi.fn().mockResolvedValue(undefined),
  };
  const props = { ...defaults, ...overrides };
  return { ...render(<Login {...props} />), props };
}

/** Real-useAuth harness — mirrors the App.tsx gate (loading → Login →
 * AuthedApp) and threads the live useAuth bundle into Login. Used by
 * the M1 regression test to drive the *real* phase machinery, which
 * is where the original prop-mock tests gave us a false sense of
 * security. The "AuthedApp" slot is a sentinel marker the test can
 * look for to confirm the gate moved past Login (or, in the M1 case,
 * did NOT move past it during the transient pending-enroll state). */
function AuthHarness() {
  const auth = useAuth();
  if (auth.phase === "loading") {
    return <div data-testid="harness-loading">loading</div>;
  }
  if (auth.phase !== "authed") {
    return (
      <Login
        phase={auth.phase}
        error={auth.error}
        onLogin={auth.login}
        onSetPassword={auth.setPassword}
        onCompleteEnrollGate={auth.completeEnrollGate}
        onClearError={auth.clearError}
        onRefreshStatus={auth.refreshStatus}
      />
    );
  }
  return <div data-testid="harness-authed">authed dashboard</div>;
}

/** Stub `fetch` for the AuthHarness tests so useAuth can run for real
 * without standing up a server. Each test installs the sequence of
 * responses it needs in order: typically `/api/auth/status` (returns
 * setup_required=true), then `/api/auth/set-password` (200 with token). */
function stubAuthFetch(...responses: Response[]) {
  const fetchMock = vi.fn();
  for (const r of responses) {
    fetchMock.mockResolvedValueOnce(r);
  }
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function jsonResponse(body: unknown, init?: { status?: number }): Response {
  return new Response(JSON.stringify(body), {
    status: init?.status ?? 200,
    headers: { "content-type": "application/json" },
  });
}

beforeEach(() => {
  // Reset both hook mocks to the default "passkey-supported" shape so
  // each test starts from a known baseline. Tests that need a different
  // shape override via mockReturnValue *after* this.
  mockedUseAuthProfile.mockReturnValue({
    profile: "local-lan",
    canEnrollPasskeyOnThisOrigin: true,
    passkeyRequired: false,
    allowEphemeralListener: true,
    loaded: true,
  });
  mockedUsePasskey.mockReturnValue({
    enroll: vi.fn().mockResolvedValue({ ok: true, credentialId: "cred-1" }),
    loginWithPasskey: vi
      .fn()
      .mockResolvedValue({ ok: true, token: "t", deviceId: "d" }),
  });
  try {
    localStorage.removeItem("wisphive-web-token");
  } catch {
    /* jsdom */
  }
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------
// Login-with-passkey button visibility
// ---------------------------------------------------------------------------

describe("Login — login-with-passkey button visibility", () => {
  it("renders 'Sign in with a passkey' when origin supports passkey and phase=unauthed", () => {
    renderLogin({ phase: "unauthed" });
    expect(screen.getByRole("button", { name: /sign in with a passkey/i })).toBeInTheDocument();
    // Password form is also still present — it's the always-available fallback.
    expect(screen.getByRole("button", { name: /^sign in$/i })).toBeInTheDocument();
  });

  it("hides 'Sign in with a passkey' when canEnrollPasskeyOnThisOrigin=false", () => {
    mockedUseAuthProfile.mockReturnValue({
      profile: "local-lan",
      canEnrollPasskeyOnThisOrigin: false,
      passkeyRequired: false,
      allowEphemeralListener: true,
      loaded: true,
    });
    renderLogin({ phase: "unauthed" });
    expect(
      screen.queryByRole("button", { name: /sign in with a passkey/i }),
    ).not.toBeInTheDocument();
    // Password form still works.
    expect(screen.getByRole("button", { name: /^sign in$/i })).toBeInTheDocument();
  });

  it("hides 'Sign in with a passkey' on phase=setup (no creds enrolled yet)", () => {
    renderLogin({ phase: "setup" });
    expect(
      screen.queryByRole("button", { name: /sign in with a passkey/i }),
    ).not.toBeInTheDocument();
    // Set-password form is what the setup phase shows.
    expect(screen.getByRole("button", { name: /set password/i })).toBeInTheDocument();
  });

  it("hides 'Sign in with a passkey' while useAuthProfile is still loading", () => {
    // loaded=false collapses the gate — don't flash a button that might
    // disappear once the probe resolves.
    mockedUseAuthProfile.mockReturnValue({
      profile: null,
      canEnrollPasskeyOnThisOrigin: false,
      passkeyRequired: false,
      allowEphemeralListener: false,
      loaded: false,
    });
    renderLogin({ phase: "unauthed" });
    expect(
      screen.queryByRole("button", { name: /sign in with a passkey/i }),
    ).not.toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// Login-with-passkey behavior
// ---------------------------------------------------------------------------

describe("Login — login-with-passkey behavior", () => {
  it("calls usePasskey.loginWithPasskey on click and surfaces inline error on failure", async () => {
    const user = userEvent.setup();
    const loginFn = vi.fn().mockResolvedValue({
      ok: false,
      error: { kind: "cancelled", message: "Passkey prompt was cancelled or timed out." },
    });
    mockedUsePasskey.mockReturnValue({
      enroll: vi.fn(),
      loginWithPasskey: loginFn,
    });
    renderLogin({ phase: "unauthed" });
    await user.click(screen.getByRole("button", { name: /sign in with a passkey/i }));
    expect(loginFn).toHaveBeenCalledTimes(1);
    // The inline error renders the per-kind message (taxonomy mapping
    // lives in Login.tsx::passkeyErrorText).
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/cancelled/i);
    });
    // Password form still rendered as fallback after passkey failure —
    // the field and submit button are present. The button is disabled
    // only because the user hasn't typed a password yet (the unrelated
    // `!password` guard from the existing onboarding), NOT because of
    // the passkey failure. The contract we pin here is "the password
    // surface is still in the DOM", not "the button is enabled
    // regardless of input" — those are independent invariants.
    expect(screen.getByLabelText(/^password$/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^sign in$/i })).toBeInTheDocument();
  });

  it("renders 'unsupported' error text for the unsupported kind", async () => {
    const user = userEvent.setup();
    mockedUsePasskey.mockReturnValue({
      enroll: vi.fn(),
      loginWithPasskey: vi.fn().mockResolvedValue({
        ok: false,
        error: { kind: "unsupported", message: "This browser does not support passkeys." },
      }),
    });
    renderLogin({ phase: "unauthed" });
    await user.click(screen.getByRole("button", { name: /sign in with a passkey/i }));
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/does not support passkeys/i);
    });
  });

  it("renders the server's verbatim message for server_rejected (e.g. counter regression)", async () => {
    const user = userEvent.setup();
    mockedUsePasskey.mockReturnValue({
      enroll: vi.fn(),
      loginWithPasskey: vi.fn().mockResolvedValue({
        ok: false,
        error: { kind: "server_rejected", message: "counter regression detected" },
      }),
    });
    renderLogin({ phase: "unauthed" });
    await user.click(screen.getByRole("button", { name: /sign in with a passkey/i }));
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/counter regression detected/);
    });
  });

  // M2 fix coverage — sudo_required renders the friendly itr#313 notice,
  // never the raw JSON discriminant.
  it("renders a friendly waiting-on-itr#313 message for sudo_required (NOT raw JSON)", async () => {
    const user = userEvent.setup();
    mockedUsePasskey.mockReturnValue({
      enroll: vi.fn().mockResolvedValue({
        ok: false,
        error: {
          kind: "sudo_required",
          // The discriminant message from the hook is intentionally
          // generic — Login.tsx::passkeyErrorText owns the user-facing
          // text. Use a clearly-server-shaped string here so the
          // assertion below distinguishes "renders friendly text" from
          // "echoes hook message verbatim".
          message: "Passkey enrollment requires re-authentication (pending itr#313).",
        },
      }),
      loginWithPasskey: vi.fn(),
    });
    // Drive the enroll path so the sudo_required surfaces in the enroll
    // card (where it'd actually appear in production).
    const props = renderLogin({ phase: "authed-pending-enroll" });
    expect(props.props.phase).toBe("authed-pending-enroll");
    await user.click(screen.getByRole("button", { name: /^enroll passkey$/i }));
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/itr#313/);
    });
    // Belt-and-braces: the user-facing string explicitly mentions
    // re-entering the password (the actionable explanation), and does
    // NOT contain the raw discriminant string `sudo_required`.
    expect(screen.getByRole("alert")).toHaveTextContent(/re-entering your password/i);
    expect(screen.getByRole("alert").textContent).not.toMatch(/sudo_required/);
  });
});

// ---------------------------------------------------------------------------
// Enroll-after-set-password step (prop-driven view tests)
// ---------------------------------------------------------------------------

describe("Login — enroll-after-set-password step (prop-driven)", () => {
  it("renders the enroll card when phase=authed-pending-enroll", () => {
    // The card is now driven directly off the parent's phase. This
    // mirrors what App.tsx (via useAuth) emits after a successful
    // setPassword on an origin that can host enrollment.
    renderLogin({ phase: "authed-pending-enroll" });
    expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /skip for now/i })).toBeInTheDocument();
    // The password form is gone (replaced by the enroll card).
    expect(screen.queryByLabelText(/new password/i)).not.toBeInTheDocument();
  });

  it("does NOT show the enroll step when setPassword fails", async () => {
    const user = userEvent.setup();
    // Failed setPassword stays in the `setup` phase — the parent's
    // useAuth never flips to authed-pending-enroll, so the card never
    // renders. We verify by checking the post-submit DOM still has
    // the set-password form, not the enroll card.
    const onSetPassword = vi.fn().mockResolvedValue(false);
    renderLogin({ phase: "setup", onSetPassword });

    await user.type(screen.getByLabelText(/new password/i), "correcthorse");
    await user.type(screen.getByLabelText(/confirm password/i), "correcthorse");
    await user.click(screen.getByRole("button", { name: /set password/i }));

    await waitFor(() => expect(onSetPassword).toHaveBeenCalled());
    expect(
      screen.queryByRole("button", { name: /^enroll passkey$/i }),
    ).not.toBeInTheDocument();
  });

  it("Skip click calls onCompleteEnrollGate (parent flips phase to authed)", async () => {
    const user = userEvent.setup();
    const onCompleteEnrollGate = vi.fn();
    renderLogin({ phase: "authed-pending-enroll", onCompleteEnrollGate });
    await user.click(screen.getByRole("button", { name: /skip for now/i }));
    expect(onCompleteEnrollGate).toHaveBeenCalledTimes(1);
  });

  it("Enroll success calls onCompleteEnrollGate", async () => {
    const user = userEvent.setup();
    const onCompleteEnrollGate = vi.fn();
    renderLogin({ phase: "authed-pending-enroll", onCompleteEnrollGate });
    await user.click(screen.getByRole("button", { name: /^enroll passkey$/i }));
    await waitFor(() => {
      expect(onCompleteEnrollGate).toHaveBeenCalledTimes(1);
    });
  });

  it("Enroll failure leaves the gate closed (no onCompleteEnrollGate call) and shows inline error", async () => {
    const user = userEvent.setup();
    const onCompleteEnrollGate = vi.fn();
    mockedUsePasskey.mockReturnValue({
      enroll: vi.fn().mockResolvedValue({
        ok: false,
        error: { kind: "cancelled", message: "Passkey prompt was cancelled or timed out." },
      }),
      loginWithPasskey: vi.fn(),
    });
    renderLogin({ phase: "authed-pending-enroll", onCompleteEnrollGate });
    await user.click(screen.getByRole("button", { name: /^enroll passkey$/i }));
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/cancelled/i);
    });
    // User stays in the enroll step (gate not released) so they can retry or skip.
    expect(onCompleteEnrollGate).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();
  });

  it("retries on inline error: enroll fails → error shown → click again works", async () => {
    const user = userEvent.setup();
    const enroll = vi
      .fn()
      .mockResolvedValueOnce({
        ok: false,
        error: { kind: "cancelled", message: "Passkey prompt was cancelled or timed out." },
      })
      .mockResolvedValueOnce({ ok: true, credentialId: "cred-1" });
    mockedUsePasskey.mockReturnValue({
      enroll,
      loginWithPasskey: vi.fn(),
    });
    const onCompleteEnrollGate = vi.fn();
    renderLogin({ phase: "authed-pending-enroll", onCompleteEnrollGate });

    // First click — error renders inline; enroll step stays put so
    // the user can retry without losing context.
    await user.click(screen.getByRole("button", { name: /^enroll passkey$/i }));
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/cancelled/i);
    });
    expect(onCompleteEnrollGate).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();

    // Second click — succeeds, gate released.
    await user.click(screen.getByRole("button", { name: /^enroll passkey$/i }));
    await waitFor(() => {
      expect(onCompleteEnrollGate).toHaveBeenCalledTimes(1);
    });
    expect(enroll).toHaveBeenCalledTimes(2);
  });
});

// ---------------------------------------------------------------------------
// M1 regression: enroll step DOES render with the real useAuth driving phase.
// ---------------------------------------------------------------------------
//
// The original tests passed `phase: "setup"` + a mocked
// `onSetPassword` that returned `true` without driving real auth state
// — so the React 19 setPhase + setPendingEnroll batch race never
// surfaced. These tests mount the full AuthHarness (real useAuth +
// real Login) and assert that a successful set-password landing in
// useAuth produces the enroll card on screen rather than dropping
// straight into the authed-dashboard sentinel.

describe("Login — M1 regression (real useAuth + Login stack)", () => {
  it("after successful setPassword on a passkey-capable origin, the enroll card renders", async () => {
    const user = userEvent.setup();
    // First call: useAuth probes /api/auth/status. We return
    // setup_required=true so the harness lands on phase=setup.
    // Second call: useAuth submits /api/auth/set-password and gets
    // back 200 with a token. useAuth should then read
    // useAuthProfile().canEnrollPasskeyOnThisOrigin (mocked true at
    // the top of this file) and emit phase=authed-pending-enroll —
    // which keeps Login mounted and renders the enroll card.
    stubAuthFetch(
      jsonResponse({ password_set: false, setup_required: true }),
      jsonResponse({ device_id: "dev-1", token: "tok-1" }),
    );

    render(<AuthHarness />);

    // Wait for the status probe to flip the harness from loading
    // through to the setup form.
    await waitFor(() => {
      expect(screen.getByLabelText(/new password/i)).toBeInTheDocument();
    });

    // Drive the form. If the bug were unfixed, the
    // setPhase("authed") + Login's local setPendingEnroll(true) would
    // batch — App.tsx's gate (which the harness mirrors) would
    // unmount Login before the enroll card rendered, and we'd see
    // the harness-authed sentinel instead.
    await user.type(screen.getByLabelText(/new password/i), "correcthorse");
    await user.type(screen.getByLabelText(/confirm password/i), "correcthorse");
    await user.click(screen.getByRole("button", { name: /set password/i }));

    // The enroll card MUST appear. This is the assertion the original
    // prop-mock test failed to verify.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();
    });
    // And the harness MUST still be in the Login surface, not the
    // authed dashboard — the gate is held open by useAuth's
    // `authed-pending-enroll` phase until Login calls completeEnrollGate.
    expect(screen.queryByTestId("harness-authed")).not.toBeInTheDocument();
  });

  it("Skip from the real-stack enroll card advances harness to the authed dashboard", async () => {
    const user = userEvent.setup();
    stubAuthFetch(
      jsonResponse({ password_set: false, setup_required: true }),
      jsonResponse({ device_id: "dev-1", token: "tok-1" }),
    );
    render(<AuthHarness />);
    await waitFor(() => {
      expect(screen.getByLabelText(/new password/i)).toBeInTheDocument();
    });
    await user.type(screen.getByLabelText(/new password/i), "correcthorse");
    await user.type(screen.getByLabelText(/confirm password/i), "correcthorse");
    await user.click(screen.getByRole("button", { name: /set password/i }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();
    });
    // Click Skip — useAuth.completeEnrollGate flips phase to authed,
    // the harness swaps to the dashboard sentinel.
    await user.click(screen.getByRole("button", { name: /skip for now/i }));
    await waitFor(() => {
      expect(screen.getByTestId("harness-authed")).toBeInTheDocument();
    });
  });

  it("when canEnrollPasskeyOnThisOrigin=false, setPassword skips straight to authed (no enroll card)", async () => {
    // Origin can't host enrollment — useAuth should NOT park in
    // authed-pending-enroll. Renders the harness-authed sentinel
    // directly after the set-password succeeds.
    mockedUseAuthProfile.mockReturnValue({
      profile: "local-lan",
      canEnrollPasskeyOnThisOrigin: false,
      passkeyRequired: false,
      allowEphemeralListener: true,
      loaded: true,
    });
    const user = userEvent.setup();
    stubAuthFetch(
      jsonResponse({ password_set: false, setup_required: true }),
      jsonResponse({ device_id: "dev-1", token: "tok-1" }),
    );
    render(<AuthHarness />);
    await waitFor(() => {
      expect(screen.getByLabelText(/new password/i)).toBeInTheDocument();
    });
    await user.type(screen.getByLabelText(/new password/i), "correcthorse");
    await user.type(screen.getByLabelText(/confirm password/i), "correcthorse");
    await user.click(screen.getByRole("button", { name: /set password/i }));
    await waitFor(() => {
      expect(screen.getByTestId("harness-authed")).toBeInTheDocument();
    });
    expect(
      screen.queryByRole("button", { name: /^enroll passkey$/i }),
    ).not.toBeInTheDocument();
  });
});
