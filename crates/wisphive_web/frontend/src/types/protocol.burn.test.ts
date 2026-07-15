import { describe, expect, it } from "vitest";
import { parseServerMessage } from "./protocol";

describe("parseServerMessage — burn_response (itr#402)", () => {
  it("parses a full frame with file and Bash touches", () => {
    const frame = JSON.stringify({
      type: "burn_response",
      touches: [
        {
          agent_id: "cc-worker-1",
          project: "/proj/alpha",
          tool_name: "Write",
          tool_input: { file_path: "/proj/alpha/src/lib.rs", content: "…" },
          ts: "2026-07-15T12:00:00Z",
        },
        {
          agent_id: "cc-worker-1",
          project: "/proj/alpha",
          tool_name: "Bash",
          tool_input: { command: "git commit -m 'feat: x'" },
          ts: "2026-07-15T12:05:00Z",
        },
      ],
    });
    const msg = parseServerMessage(frame);
    if (msg.type !== "burn_response") throw new Error("wrong variant");
    expect(msg.touches).toHaveLength(2);
    expect(msg.touches[0].tool_name).toBe("Write");
    expect(msg.touches[0].tool_input).toEqual({
      file_path: "/proj/alpha/src/lib.rs",
      content: "…",
    });
    expect(msg.touches[1].ts).toBe("2026-07-15T12:05:00Z");
  });

  it("parses a minimal frame (tool_input elided) — additive-fields contract", () => {
    const frame = JSON.stringify({
      type: "burn_response",
      touches: [
        { agent_id: "cc-1", project: "/p", tool_name: "Edit", ts: "2026-07-15T00:00:00Z" },
      ],
    });
    const msg = parseServerMessage(frame);
    if (msg.type !== "burn_response") throw new Error("wrong variant");
    expect(msg.touches[0].tool_input).toBeUndefined();
  });

  it("rejects malformed touches at the trust boundary", () => {
    const frame = JSON.stringify({
      type: "burn_response",
      touches: [{ agent_id: 42, project: "/p", tool_name: "Edit", ts: "2026-07-15T00:00:00Z" }],
    });
    expect(() => parseServerMessage(frame)).toThrow(/expected string/);
  });
});
