import { describe, expect, it } from "vitest";
import { parseServerMessage } from "./protocol";

const DECISION = {
  id: "00000000-0000-4000-8000-000000000042",
  agent_id: "managed-codex",
  agent_type: "codex",
  project: "/proj",
  tool_name: "SpawnAgent",
  tool_input: { project: "/proj", prompt: "do it" },
  timestamp: "2026-07-18T12:00:00Z",
  hook_event_name: "PreToolUse",
};

describe("parseServerMessage — correlated spawn ack and errors (itr#567)", () => {
  it("parses agent_spawn_queued with its decision and correlation id", () => {
    const frame = JSON.stringify({
      type: "agent_spawn_queued",
      decision: DECISION,
      correlation_id: "spawn-corr-1",
    });
    const msg = parseServerMessage(frame);
    if (msg.type !== "agent_spawn_queued") throw new Error("wrong variant");
    expect(msg.decision.id).toBe(DECISION.id);
    expect(msg.decision.tool_name).toBe("SpawnAgent");
    expect(msg.correlation_id).toBe("spawn-corr-1");
  });

  it("parses agent_spawn_queued without a correlation id (elided on the wire)", () => {
    const frame = JSON.stringify({ type: "agent_spawn_queued", decision: DECISION });
    const msg = parseServerMessage(frame);
    if (msg.type !== "agent_spawn_queued") throw new Error("wrong variant");
    expect(msg.correlation_id).toBeUndefined();
  });

  it("parses command_error with and without a correlation id", () => {
    const correlated = parseServerMessage(
      JSON.stringify({
        type: "command_error",
        message: "invalid agent spawn request: prompt must not be empty",
        correlation_id: "spawn-corr-2",
      }),
    );
    if (correlated.type !== "command_error") throw new Error("wrong variant");
    expect(correlated.message).toBe("invalid agent spawn request: prompt must not be empty");
    expect(correlated.correlation_id).toBe("spawn-corr-2");

    const bare = parseServerMessage(
      JSON.stringify({ type: "command_error", message: "boom" }),
    );
    if (bare.type !== "command_error") throw new Error("wrong variant");
    expect(bare.correlation_id).toBeUndefined();
  });

  it("rejects a command_error without a message at the trust boundary", () => {
    expect(() =>
      parseServerMessage(JSON.stringify({ type: "command_error", correlation_id: "x" })),
    ).toThrow(/message/);
  });

  it("rejects agent_spawn_queued with a malformed decision", () => {
    const frame = JSON.stringify({
      type: "agent_spawn_queued",
      decision: { ...DECISION, id: "not-a-uuid" },
      correlation_id: "spawn-corr-3",
    });
    expect(() => parseServerMessage(frame)).toThrow(/UUID/);
  });
});
