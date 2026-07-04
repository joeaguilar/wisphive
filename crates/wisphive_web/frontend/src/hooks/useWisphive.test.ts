import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useWisphive } from "./useWisphive";
import type { ProjectHookStatus } from "../types/protocol";

// Minimal WebSocket stand-in: records outbound frames and lets the test
// drive onopen/onmessage. useWisphive keys `send` off `readyState ===
// WebSocket.OPEN`, so OPEN must be 1 and `open()` must flip readyState.
class MockWebSocket {
  static instances: MockWebSocket[] = [];
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  url: string;
  readyState = MockWebSocket.CONNECTING;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: ((e: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  send(data: string) {
    this.sent.push(data);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
  }

  // ── test helpers ──
  open() {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.();
  }

  emit(msg: unknown) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }

  sentMessages(): Array<Record<string, unknown>> {
    return this.sent.map((s) => JSON.parse(s));
  }
}

function latest(): MockWebSocket {
  return MockWebSocket.instances[MockWebSocket.instances.length - 1];
}

const PROJECT = "/Users/j/controller";

function status(overrides: Partial<ProjectHookStatus> = {}): ProjectHookStatus {
  return {
    project: PROJECT,
    mode: "active",
    claude_installed: true,
    codex_installed: true,
    missing_events: [],
    all_installed: true,
    all_enabled: true,
    ...overrides,
  };
}

describe("useWisphive hook-gating (itr#460)", () => {
  beforeEach(() => {
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket as unknown as typeof WebSocket);
    localStorage.setItem("wisphive-web-token", "test-token");
  });

  afterEach(() => {
    cleanup();
    localStorage.clear();
    vi.unstubAllGlobals();
  });

  async function mountOpen() {
    const view = renderHook(() => useWisphive());
    await waitFor(() => expect(MockWebSocket.instances.length).toBeGreaterThan(0));
    act(() => latest().open());
    return view;
  }

  it("installHooks sends install_hooks for the given project", async () => {
    const { result } = await mountOpen();
    act(() => result.current.installHooks(PROJECT));
    expect(latest().sentMessages()).toContainEqual({ type: "install_hooks", project: PROJECT });
  });

  it("queryProjectHookStatus sends query_project_hook_status", async () => {
    const { result } = await mountOpen();
    act(() => result.current.queryProjectHookStatus(PROJECT));
    expect(latest().sentMessages()).toContainEqual({
      type: "query_project_hook_status",
      project: PROJECT,
    });
  });

  it("project_hook_status merges into the hookStatus map keyed by project", async () => {
    const { result } = await mountOpen();
    act(() => latest().emit({ type: "project_hook_status", ...status() }));
    expect(result.current.hookStatus[PROJECT]).toEqual(status());
  });

  it("install_hooks_result success updates the badge status and clears any error", async () => {
    const { result } = await mountOpen();
    // Seed an error first.
    act(() => latest().emit({ type: "install_hooks_result", project: PROJECT, error: "boom" }));
    expect(result.current.hookErrors[PROJECT]).toBe("boom");

    act(() =>
      latest().emit({ type: "install_hooks_result", project: PROJECT, status: status() }),
    );
    expect(result.current.hookStatus[PROJECT]).toEqual(status());
    expect(result.current.hookErrors[PROJECT]).toBeUndefined();
  });

  it("install_hooks_result error surfaces a per-project message", async () => {
    const { result } = await mountOpen();
    act(() =>
      latest().emit({
        type: "install_hooks_result",
        project: PROJECT,
        error: "settings.json not writable",
      }),
    );
    expect(result.current.hookErrors[PROJECT]).toBe("settings.json not writable");
  });

  it("an InstallHooks reauth bounce stashes and, on reauth success, replays install_hooks", async () => {
    const { result } = await mountOpen();

    // 1. User installs — sudo-gated, so the daemon will bounce it.
    act(() => result.current.installHooks(PROJECT));
    const firstCount = latest().sentMessages().filter((m) => m.type === "install_hooks").length;
    expect(firstCount).toBe(1);

    // 2. Daemon bounces with a reauth keyed by project path + InstallHooks.
    act(() =>
      latest().emit({
        type: "web_reauth_required",
        device_id: "dev-1",
        request_id: PROJECT,
        tool_name: "InstallHooks",
        at: "2026-07-04T12:00:00Z",
      }),
    );
    expect(result.current.pendingReauth).toEqual({
      request_id: PROJECT,
      tool_name: "InstallHooks",
    });

    // 3. SudoModal success → replay the exact install for the stashed project.
    act(() => result.current.retryPendingApprove());
    expect(result.current.pendingReauth).toBeNull();
    const replays = latest().sentMessages().filter(
      (m) => m.type === "install_hooks" && m.project === PROJECT,
    );
    expect(replays.length).toBe(2);
  });

  it("dismissing an InstallHooks reauth drops the stash so success cannot replay", async () => {
    const { result } = await mountOpen();
    act(() => result.current.installHooks(PROJECT));
    act(() =>
      latest().emit({
        type: "web_reauth_required",
        device_id: "dev-1",
        request_id: PROJECT,
        tool_name: "InstallHooks",
        at: "2026-07-04T12:00:00Z",
      }),
    );
    act(() => result.current.dismissReauth());
    expect(result.current.pendingReauth).toBeNull();

    // A stray retry after dismiss must not re-send the install.
    act(() => result.current.retryPendingApprove());
    const installs = latest().sentMessages().filter((m) => m.type === "install_hooks");
    expect(installs.length).toBe(1);
  });
});
