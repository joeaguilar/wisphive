/**
 * Login.tsx — Vitest coverage of the passkey integration (#312).
 *
 * The component is driven by two hooks (`useAuthProfile`, `usePasskey`)
 * + four parent callbacks. We mock the hooks at the module boundary so
 * we can independently flip `canEnrollPasskeyOnThisOrigin` and the
 * enroll/login outcomes without standing up a full fetch fake. The
 * pre-existing password-form behavior (validated by manual smoke since
 * itr#217) is exercised indirectly via the "passkey UI does/doesn't
 * hide the password form" tests — the form keeps working as the
 * always-available fallback regardless of profile state.
 *
 * What this file pins:
 *   - Login-with-passkey button shown only when canEnrollPasskeyOnThisOrigin=true
 *     AND phase=unauthed (never on setup phase — no creds enrolled yet)
 *   - Login-with-passkey button hidden when canEnrollPasskeyOnThisOrigin=false
 *   - Successful setPassword + canEnrollPasskeyOnThisOrigin=true →
 *     enroll step appears; Skip + Retry transitions; success returns to
 *     authed dashboard (parent unmount).
 *   - Failed setPassword does NOT enter the enroll step.
 *   - canEnrollPasskeyOnThisOrigin=false → setPassword bypasses the
 *     enroll step entirely (returns straight to whatever the parent
 *     renders for authed).
 *   - Passkey errors render the per-kind inline message.
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
import { useAuthProfile } from "../hooks/useAuthProfile";
import { usePasskey } from "../hooks/usePasskey";

const mockedUseAuthProfile = vi.mocked(useAuthProfile);
const mockedUsePasskey = vi.mocked(usePasskey);

/** Build a Props bundle with sensible defaults — individual tests
 * override the bits they care about. Keeps each `render` call small
 * and intention-revealing. */
function renderLogin(overrides: Partial<React.ComponentProps<typeof Login>> = {}) {
  const defaults: React.ComponentProps<typeof Login> = {
    phase: "unauthed",
    error: null,
    onLogin: vi.fn().mockResolvedValue(true),
    onSetPassword: vi.fn().mockResolvedValue(true),
    onClearError: vi.fn(),
    onRefreshStatus: vi.fn().mockResolvedValue(undefined),
  };
  const props = { ...defaults, ...overrides };
  return { ...render(<Login {...props} />), props };
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
});

afterEach(() => {
  cleanup();
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
});

// ---------------------------------------------------------------------------
// Enroll-after-set-password step
// ---------------------------------------------------------------------------

describe("Login — enroll-after-set-password step", () => {
  it("shows the enroll step after successful setPassword when origin supports passkey", async () => {
    const user = userEvent.setup();
    const onSetPassword = vi.fn().mockResolvedValue(true);
    renderLogin({ phase: "setup", onSetPassword });

    await user.type(screen.getByLabelText(/new password/i), "correcthorse");
    await user.type(screen.getByLabelText(/confirm password/i), "correcthorse");
    await user.click(screen.getByRole("button", { name: /set password/i }));

    // Enroll step renders — primary CTA + Skip both present.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();
    });
    expect(screen.getByRole("button", { name: /skip for now/i })).toBeInTheDocument();
    // Password form is gone (replaced by the enroll card).
    expect(screen.queryByLabelText(/new password/i)).not.toBeInTheDocument();
  });

  it("skips the enroll step when canEnrollPasskeyOnThisOrigin=false", async () => {
    const user = userEvent.setup();
    mockedUseAuthProfile.mockReturnValue({
      profile: "local-lan",
      canEnrollPasskeyOnThisOrigin: false,
      passkeyRequired: false,
      allowEphemeralListener: true,
      loaded: true,
    });
    const onSetPassword = vi.fn().mockResolvedValue(true);
    renderLogin({ phase: "setup", onSetPassword });

    await user.type(screen.getByLabelText(/new password/i), "correcthorse");
    await user.type(screen.getByLabelText(/confirm password/i), "correcthorse");
    await user.click(screen.getByRole("button", { name: /set password/i }));

    // Enroll step MUST NOT render — the LAN-IP origin case ends here
    // and the parent will swap to the authed dashboard.
    await waitFor(() => {
      expect(onSetPassword).toHaveBeenCalled();
    });
    expect(
      screen.queryByRole("button", { name: /^enroll passkey$/i }),
    ).not.toBeInTheDocument();
  });

  it("does NOT show the enroll step when setPassword fails", async () => {
    const user = userEvent.setup();
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

  it("skip dismisses the enroll step", async () => {
    const user = userEvent.setup();
    renderLogin({ phase: "setup" });

    await user.type(screen.getByLabelText(/new password/i), "correcthorse");
    await user.type(screen.getByLabelText(/confirm password/i), "correcthorse");
    await user.click(screen.getByRole("button", { name: /set password/i }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: /skip for now/i }));

    // Enroll step gone; ENROLL button no longer present.
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: /^enroll passkey$/i }),
      ).not.toBeInTheDocument();
    });
    // We DON'T re-render the password form (parent gate would have
    // moved phase to "authed" by now in production — the local
    // component just falls through to the unauthed render path).
    // What we CAN assert: the test's enroll step element specifically
    // vanished, which is the user-visible contract.
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
    renderLogin({ phase: "setup" });

    await user.type(screen.getByLabelText(/new password/i), "correcthorse");
    await user.type(screen.getByLabelText(/confirm password/i), "correcthorse");
    await user.click(screen.getByRole("button", { name: /set password/i }));

    // First click — error renders inline; enroll step stays put so
    // the user can retry without losing context.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: /^enroll passkey$/i }));
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/cancelled/i);
    });
    // Step is still present — the button is the retry affordance.
    expect(screen.getByRole("button", { name: /^enroll passkey$/i })).toBeInTheDocument();

    // Second click — succeeds, step disappears.
    await user.click(screen.getByRole("button", { name: /^enroll passkey$/i }));
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: /^enroll passkey$/i }),
      ).not.toBeInTheDocument();
    });
    expect(enroll).toHaveBeenCalledTimes(2);
  });
});
