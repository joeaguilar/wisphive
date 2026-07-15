import { describe, expect, it } from "vitest";
import { parseServerMessage } from "./protocol";

describe("parseServerMessage — worktrees_response (itr#401)", () => {
  it("parses a full frame with changes, attribution, and branch metadata", () => {
    const frame = JSON.stringify({
      type: "worktrees_response",
      worktrees: [
        {
          project: "/proj/alpha",
          is_git_repo: true,
          branch: "main",
          detached: false,
          head: "abc123def4567890abc123def4567890abc123de",
          upstream: "origin/main",
          ahead: 2,
          behind: 1,
          changes: [
            {
              path: "src/lib.rs",
              status: ".M",
              attributed_to: "cc-worker-1",
              attributed_tool: "Edit",
            },
            { path: "b.txt", status: "R.", orig_path: "a.txt" },
          ],
          changes_truncated: false,
          diffstat: "1 file changed, 3 insertions(+)",
          probed_at: "2026-07-15T12:00:00Z",
        },
      ],
    });
    const msg = parseServerMessage(frame);
    if (msg.type !== "worktrees_response") throw new Error("wrong variant");
    expect(msg.worktrees).toHaveLength(1);
    const wt = msg.worktrees[0];
    expect(wt.branch).toBe("main");
    expect(wt.ahead).toBe(2);
    expect(wt.changes[0].attributed_to).toBe("cc-worker-1");
    expect(wt.changes[1].orig_path).toBe("a.txt");
  });

  it("parses a minimal frame (optional wire fields elided) with safe defaults", () => {
    const frame = JSON.stringify({
      type: "worktrees_response",
      worktrees: [
        { project: "/p", is_git_repo: false, probed_at: "2026-07-15T00:00:00Z" },
      ],
    });
    const msg = parseServerMessage(frame);
    if (msg.type !== "worktrees_response") throw new Error("wrong variant");
    const wt = msg.worktrees[0];
    expect(wt.is_git_repo).toBe(false);
    expect(wt.detached).toBe(false);
    expect(wt.changes).toEqual([]);
    expect(wt.changes_truncated).toBe(false);
    expect(wt.branch).toBeUndefined();
  });

  it("rejects malformed change entries at the trust boundary", () => {
    const frame = JSON.stringify({
      type: "worktrees_response",
      worktrees: [
        {
          project: "/p",
          is_git_repo: true,
          probed_at: "2026-07-15T00:00:00Z",
          changes: [{ path: 42, status: ".M" }],
        },
      ],
    });
    expect(() => parseServerMessage(frame)).toThrow(/expected string/);
  });
});
