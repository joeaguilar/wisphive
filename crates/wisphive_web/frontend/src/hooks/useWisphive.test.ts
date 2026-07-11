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

  emitRaw(data: string) {
    this.onmessage?.({ data });
  }

  sentMessages(): Array<Record<string, unknown>> {
    return this.sent.map((s) => JSON.parse(s));
  }
}

function latest(): MockWebSocket {
  const socket = MockWebSocket.instances.at(-1);
  if (!socket) throw new Error("expected a WebSocket instance");
  return socket;
}

const PROJECT = "/Users/j/controller";
const REQUEST_ID = "00000000-0000-4000-8000-000000000001";
const VALID_REQUEST_ID = "00000000-0000-4000-8000-000000000002";
const BASH_REQUEST_ID = "00000000-0000-4000-8000-000000000003";
const READ_REQUEST_ID = "00000000-0000-4000-8000-000000000004";
const HISTORY_ID = "00000000-0000-4000-8000-000000000005";
const TERMINAL_ID = "00000000-0000-4000-8000-000000000006";

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

function decisionRequest(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    id: REQUEST_ID,
    agent_id: "cc-1",
    agent_type: "claude_code",
    project: "/proj",
    tool_name: "Read",
    tool_input: { file_path: "/proj/README.md" },
    timestamp: "2026-07-11T12:00:00Z",
    hook_event_name: "PreToolUse",
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

  it("rejects malformed and schema-invalid frames without breaking later valid messages", async () => {
    const warning = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const { result } = await mountOpen();

    const invalidFrames = [
      { type: "new_decision", ...decisionRequest({ id: 42 }) },
      { type: "new_decision", ...decisionRequest({ agent_type: "chatgpt" }) },
      { type: "new_decision", ...decisionRequest({ id: "not-a-uuid" }) },
      {
        type: "new_decision",
        ...decisionRequest({ timestamp: "2026-02-30T12:00:00Z" }),
      },
      { type: "new_decision", ...decisionRequest({ hook_event_name: "ToolMaybe" }) },
      { type: "welcome", version: 4_294_967_296 },
      { type: "agent_exited", agent_id: "managed-1", exit_code: 2_147_483_648 },
      {
        type: "term_catchup",
        id: TERMINAL_ID,
        cols: 65_536,
        rows: 24,
        next_seq: 0,
        screen: "",
      },
      {
        type: "term_chunk",
        id: TERMINAL_ID,
        seq: Number.MAX_SAFE_INTEGER + 1,
        ts_us: 0,
        direction: "output",
        data: "",
      },
      {
        type: "term_replay_chunk",
        id: TERMINAL_ID,
        seq: 0,
        ts_us: Number.MIN_SAFE_INTEGER - 1,
        direction: "output",
        data: "",
      },
    ];

    act(() => {
      latest().emitRaw('{"type":"new_decision"');
      for (const frame of invalidFrames) latest().emit(frame);
    });

    expect(result.current.queue).toEqual([]);
    expect(warning).toHaveBeenCalledTimes(invalidFrames.length + 1);
    expect(warning.mock.calls[0]?.[0]).toBe("Rejected invalid server message:");
    expect(warning).toHaveBeenCalledWith(
      "Rejected invalid server message:",
      expect.objectContaining({ message: "message.agent_type: expected known AgentType value" }),
    );
    expect(warning).toHaveBeenCalledWith(
      "Rejected invalid server message:",
      expect.objectContaining({ message: "message.id: expected hyphenated UUID" }),
    );
    expect(warning).toHaveBeenCalledWith(
      "Rejected invalid server message:",
      expect.objectContaining({ message: "message.timestamp: expected RFC 3339 timestamp" }),
    );
    expect(warning).toHaveBeenCalledWith(
      "Rejected invalid server message:",
      expect.objectContaining({ message: "message.version: expected u32 integer" }),
    );
    expect(warning).toHaveBeenCalledWith(
      "Rejected invalid server message:",
      expect.objectContaining({ message: "message.seq: expected u64 (JavaScript-safe) integer" }),
    );

    act(() =>
      latest().emit({
        type: "new_decision",
        ...decisionRequest({ id: VALID_REQUEST_ID, tool_input: { command: "pwd" } }),
      }),
    );

    expect(result.current.queue).toEqual([
      expect.objectContaining({ id: VALID_REQUEST_ID, tool_input: { command: "pwd" } }),
    ]);

    // Known protocol variants that this hook does not render are still
    // validated and ignored, rather than mislabeled as wire drift.
    act(() =>
      latest().emit({
        type: "agent_spawned",
        agent_id: "managed-1",
        agent_type: "codex",
        pid: 1234,
        project: "/proj",
        model: null,
        name: null,
        started_at: "2026-07-11T12:00:00Z",
        reasoning: null,
        max_turns: null,
        permission_mode: null,
      }),
    );
    expect(warning).toHaveBeenCalledTimes(invalidFrames.length + 1);
  });

  it("preserves every serde_json::Value shape across inbound payload fields", async () => {
    const { result } = await mountOpen();

    act(() =>
      latest().emit({
        type: "new_decision",
        ...decisionRequest({
          tool_input: "scalar input",
          event_data: [1, null, { nested: true }],
        }),
      }),
    );
    expect(result.current.queue.at(0)?.tool_input).toBe("scalar input");
    expect(result.current.queue.at(0)?.event_data).toEqual([1, null, { nested: true }]);

    const prototypeKeyInput = Object.fromEntries([
      ["__proto__", { command: "inherited command must stay data" }],
      ["safe", true],
    ]);
    act(() =>
      latest().emit({
        type: "new_decision",
        ...decisionRequest({
          id: "00000000-0000-4000-8000-000000000004",
          tool_input: prototypeKeyInput,
        }),
      }),
    );
    const preserved = result.current.queue.at(1)?.tool_input;
    expect(typeof preserved).toBe("object");
    expect(preserved).not.toBeNull();
    if (typeof preserved !== "object" || preserved === null || Array.isArray(preserved)) {
      throw new Error("expected preserved JSON object");
    }
    expect(Object.hasOwn(preserved, "__proto__")).toBe(true);
    expect(Object.hasOwn(preserved, "command")).toBe(false);
    expect(Object.getPrototypeOf(preserved)).toBe(Object.prototype);

    act(() =>
      latest().emit({
        type: "history_response",
        entries: [
          {
            id: HISTORY_ID,
            agent_id: "cc-1",
            agent_type: "claude_code",
            project: "/proj",
            tool_name: "Read",
            tool_input: null,
            decision: "approve",
            requested_at: "2026-07-11T07:00:00-05:00",
            resolved_at: "2026-07-11T12:00:01.123456Z",
            tool_result: [false, 7, { ok: true }],
          },
        ],
      }),
    );
    expect(result.current.history.at(0)?.tool_input).toBeNull();
    expect(result.current.history.at(0)?.tool_result).toEqual([false, 7, { ok: true }]);

    act(() =>
      latest().emit({
        type: "audit_decision",
        kind: "deferred",
        decided_by: "always_ask:intrinsic",
        project: "/proj",
        agent_id: "cc-1",
        tool_name: "AskUserQuestion",
        ts: "2026-07-11T12:00:02Z",
        tool_input: true,
      }),
    );
    expect(result.current.auditDecisions.at(0)?.tool_input).toBe(true);
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

  it("deferred_resolved removes the matching deferred row, leaving others (itr#461)", async () => {
    const { result } = await mountOpen();

    const mk = (tool_use_id: string) => ({
      kind: "deferred" as const,
      decided_by: "always_ask:intrinsic",
      project: "/proj",
      agent_id: "cc-1",
      tool_name: "AskUserQuestion",
      ts: "2026-07-04T11:58:00Z",
      tool_use_id,
    });

    act(() =>
      latest().emit({ type: "audit_snapshot", items: [mk("toolu_a"), mk("toolu_b")] }),
    );
    expect(result.current.auditDecisions).toHaveLength(2);

    act(() =>
      latest().emit({
        type: "deferred_resolved",
        tool_use_id: "toolu_a",
        agent_id: "cc-1",
        tool_name: "AskUserQuestion",
        ts: "2026-07-04T12:00:00Z",
        answer_summary: "Hey there!",
      }),
    );

    // Only the answered row is dropped; the other still waits.
    expect(result.current.auditDecisions).toHaveLength(1);
    expect(result.current.auditDecisions.at(0)?.tool_use_id).toBe("toolu_b");
  });

  it("deferred_resolved for an unknown tool_use_id leaves all rows intact (itr#461)", async () => {
    const { result } = await mountOpen();
    act(() =>
      latest().emit({
        type: "audit_snapshot",
        items: [
          {
            kind: "deferred",
            decided_by: "always_ask:intrinsic",
            project: "/proj",
            agent_id: "cc-1",
            tool_name: "AskUserQuestion",
            ts: "2026-07-04T11:58:00Z",
            tool_use_id: "toolu_keep",
          },
        ],
      }),
    );
    act(() =>
      latest().emit({
        type: "deferred_resolved",
        tool_use_id: "toolu_nope",
        agent_id: "cc-1",
        tool_name: "AskUserQuestion",
        ts: "2026-07-04T12:00:00Z",
      }),
    );
    expect(result.current.auditDecisions).toHaveLength(1);
  });

  it("agent_disconnected records a gone session; reconnect clears it (itr#464)", async () => {
    const { result } = await mountOpen();

    act(() => latest().emit({ type: "agent_disconnected", agent_id: "cc-gone" }));
    expect(result.current.endedAgentIds).toContain("cc-gone");

    // A repeat disconnect must not duplicate the id.
    act(() => latest().emit({ type: "agent_disconnected", agent_id: "cc-gone" }));
    expect(result.current.endedAgentIds.filter((id) => id === "cc-gone")).toHaveLength(1);

    // Reconnecting the same agent clears the gone flag.
    act(() =>
      latest().emit({
        type: "agent_connected",
        agent_id: "cc-gone",
        agent_type: "claude_code",
        project: "/proj",
        connected_at: "2026-07-04T12:00:00Z",
        last_seen: "2026-07-04T12:00:00Z",
      }),
    );
    expect(result.current.endedAgentIds).not.toContain("cc-gone");
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

describe("useWisphive approve-stash tool-name cross-check (itr#275)", () => {
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

  it("replays a matching sudo-class approve after reauth (regression guard alongside the InstallHooks path)", async () => {
    const { result } = await mountOpen();
    const req = decisionRequest({ id: BASH_REQUEST_ID, tool_name: "Bash" });
    act(() => latest().emit({ type: "queue_snapshot", items: [req] }));

    act(() => result.current.approve(BASH_REQUEST_ID));
    expect(latest().sentMessages().filter((m) => m.type === "approve")).toHaveLength(1);

    // Daemon bounces this Bash approve — sudo-class, so a genuine gate.
    act(() =>
      latest().emit({
        type: "web_reauth_required",
        device_id: "dev-1",
        request_id: BASH_REQUEST_ID,
        tool_name: "Bash",
        at: "2026-07-11T12:00:01Z",
      }),
    );
    expect(result.current.pendingReauth).toEqual({
      request_id: BASH_REQUEST_ID,
      tool_name: "Bash",
    });

    // Reauth succeeds and the gated tool matches what we stashed, so the
    // approve replays.
    act(() => result.current.retryPendingApprove());
    expect(result.current.pendingReauth).toBeNull();
    const replays = latest()
      .sentMessages()
      .filter((m) => m.type === "approve" && m.id === BASH_REQUEST_ID);
    expect(replays).toHaveLength(2);
  });

  it("does not replay a Read approve when the WebReauthRequired references a mismatched tool", async () => {
    const { result } = await mountOpen();
    const req = decisionRequest({ id: READ_REQUEST_ID, tool_name: "Read" });
    act(() => latest().emit({ type: "queue_snapshot", items: [req] }));

    // Submit the Read approve — useWisphive stashes {toolName: "Read", opts}
    // keyed by request_id, same bookkeeping as any other approve.
    act(() => result.current.approve(READ_REQUEST_ID));
    expect(latest().sentMessages().filter((m) => m.type === "approve")).toHaveLength(1);

    // Synthesize a stale/out-of-band WebReauthRequired: it reuses this
    // request_id but names a different (sudo-class) tool. The daemon only
    // ever emits this event synchronously from a sudo-class gated arm today
    // — a genuine message could never claim to be gating "Read" — but
    // itr#255/itr#275 flagged that any future reauth-triggering path
    // (admin-triggered reauth, session expiry, a background freshness
    // check) risks a stale/reused request_id bleeding into the stash. This
    // simulates exactly that.
    act(() =>
      latest().emit({
        type: "web_reauth_required",
        device_id: "dev-1",
        request_id: READ_REQUEST_ID,
        tool_name: "Bash",
        at: "2026-07-11T12:00:01Z",
      }),
    );
    expect(result.current.pendingReauth).toEqual({
      request_id: READ_REQUEST_ID,
      tool_name: "Bash",
    });

    // Reauth "succeeds" — but the stashed tool_name ("Read") doesn't match
    // what this WebReauthRequired claims to be gating ("Bash"), so
    // retryPendingApprove must refuse to replay the Read approve.
    act(() => result.current.retryPendingApprove());
    expect(result.current.pendingReauth).toBeNull();
    const approves = latest().sentMessages().filter((m) => m.type === "approve");
    expect(approves).toHaveLength(1); // only the original submit; no replay
  });

  it("does not replay when no approve was ever stashed for the reauth's request_id", async () => {
    const { result } = await mountOpen();
    // No queue_snapshot, no approve() call — the stash is empty for this id.
    act(() =>
      latest().emit({
        type: "web_reauth_required",
        device_id: "dev-1",
        request_id: "req-unknown",
        tool_name: "Bash",
        at: "2026-07-11T12:00:01Z",
      }),
    );
    expect(result.current.pendingReauth).toEqual({
      request_id: "req-unknown",
      tool_name: "Bash",
    });

    act(() => result.current.retryPendingApprove());
    expect(result.current.pendingReauth).toBeNull();
    expect(latest().sentMessages().filter((m) => m.type === "approve")).toHaveLength(0);
  });
});
