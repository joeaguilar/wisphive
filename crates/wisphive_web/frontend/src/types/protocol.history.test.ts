import { describe, expect, it } from "vitest";
import { knownAgentType, parseServerMessage } from "./protocol";

// itr#607: decision_log-backed rows can carry agent_type labels outside the
// closed enum (itr#562 `unrecognized:<raw>` refusal rows, arbitrary bytes).
// Before this fix `readAgentType` threw inside `parseServerMessage`, which
// dropped the ENTIRE history/sessions message — every row, not just the odd
// one. These tests pin the tolerant boundary.

const HISTORY_ENTRY = {
  id: "00000000-0000-4000-8000-000000000042",
  agent_id: "mystery-1",
  agent_type: "unrecognized:[31mghost",
  project: "/proj",
  tool_name: "Bash",
  tool_input: { command: "ls" },
  decision: "deny",
  requested_at: "2026-07-17T00:00:00Z",
  resolved_at: "2026-07-17T00:00:01Z",
  decided_by: "agent_type:unrecognized",
  config_hash: "abc123",
};

describe("parseServerMessage — non-enum agent_type rows survive (itr#607)", () => {
  it("keeps history entries whose agent_type is outside the closed enum", () => {
    const frame = JSON.stringify({
      type: "history_response",
      entries: [
        HISTORY_ENTRY,
        { ...HISTORY_ENTRY, id: "00000000-0000-4000-8000-000000000043", agent_type: "claude_code" },
      ],
    });
    const msg = parseServerMessage(frame);
    if (msg.type !== "history_response") throw new Error("wrong variant");
    expect(msg.entries).toHaveLength(2);
    expect(msg.entries[0].agent_type).toBe("unrecognized:[31mghost");
    expect(msg.entries[0].decided_by).toBe("agent_type:unrecognized");
    expect(msg.entries[1].agent_type).toBe("claude_code");
  });

  it("keeps session summaries whose agent_type is outside the closed enum", () => {
    const frame = JSON.stringify({
      type: "sessions_response",
      sessions: [
        {
          agent_id: "mystery-1",
          agent_type: "unrecognized:ghost",
          project: "/proj",
          first_seen: "2026-07-17T00:00:00Z",
          last_seen: "2026-07-17T00:00:01Z",
          total_calls: 1,
          approved: 0,
          denied: 1,
          is_live: false,
          pending_count: 0,
        },
      ],
    });
    const msg = parseServerMessage(frame);
    if (msg.type !== "sessions_response") throw new Error("wrong variant");
    expect(msg.sessions).toHaveLength(1);
    expect(msg.sessions[0].agent_type).toBe("unrecognized:ghost");
  });

  it("still rejects a non-string agent_type at the trust boundary", () => {
    const frame = JSON.stringify({
      type: "history_response",
      entries: [{ ...HISTORY_ENTRY, agent_type: 7 }],
    });
    expect(() => parseServerMessage(frame)).toThrow(/string/);
  });
});

describe("knownAgentType", () => {
  it("narrows the closed set and rejects everything else", () => {
    expect(knownAgentType("codex")).toBe("codex");
    expect(knownAgentType("claude_code")).toBe("claude_code");
    expect(knownAgentType("red")).toBe("red");
    expect(knownAgentType("local_llm")).toBe("local_llm");
    expect(knownAgentType("unrecognized:ghost")).toBeNull();
    expect(knownAgentType("")).toBeNull();
  });
});
