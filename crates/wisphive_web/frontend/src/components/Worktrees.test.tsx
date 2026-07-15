import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Worktrees } from "./Worktrees";
import { generateCommitMessage } from "./commitMessage";
import type { WorktreeChange, WorktreeStatus } from "../types/protocol";

function change(path: string, status = ".M", overrides: Partial<WorktreeChange> = {}): WorktreeChange {
  return { path, status, ...overrides };
}

function tree(overrides: Partial<WorktreeStatus>): WorktreeStatus {
  return {
    project: "/proj/alpha",
    is_git_repo: true,
    branch: "main",
    detached: false,
    head: "abc123def456",
    changes: [],
    changes_truncated: false,
    probed_at: "2026-07-15T12:00:00Z",
    ...overrides,
  };
}

const DIRTY_ALPHA = tree({
  project: "/proj/alpha",
  changes: [
    change("crates/wisphive_daemon/src/state/summaries.rs", ".M", {
      attributed_to: "cc-q3-worker",
      attributed_tool: "Edit",
    }),
    change("notes/scratchpad-with-a-quite-long-filename.txt", "??"),
  ],
  diffstat: "1 file changed, 12 insertions(+), 3 deletions(-)",
});

const DIRTY_BRAVO = tree({
  project: "/proj/bravo",
  branch: "feature/strip",
  upstream: "origin/feature/strip",
  ahead: 2,
  behind: 1,
  changes: [change("docs/guide.md", ".M")],
});

function renderStrip(worktrees: WorktreeStatus[], onLoad = vi.fn()) {
  return render(<Worktrees worktrees={worktrees} onLoad={onLoad} />);
}

describe("Worktrees", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("renders both dirty repos with branch, dirty count, ahead/behind, and diffstat", () => {
    renderStrip([DIRTY_ALPHA, DIRTY_BRAVO]);

    expect(screen.getByRole("status")).toHaveTextContent("2 dirty · 0 clean · 0 not git");

    const alpha = screen.getByRole("article", { name: "Working tree /proj/alpha" });
    expect(within(alpha).getByText("main")).toBeInTheDocument();
    expect(
      within(alpha).getByText(/2 changed files · 1 agent-attributed · 1 file changed/),
    ).toBeInTheDocument();

    const bravo = screen.getByRole("article", { name: "Working tree /proj/bravo" });
    expect(within(bravo).getByText("feature/strip")).toBeInTheDocument();
    expect(within(bravo).getByText("↑2 ↓1")).toBeInTheDocument();
  });

  it("shows FULL untruncated project paths and change paths (no-truncation rule)", () => {
    renderStrip([DIRTY_ALPHA]);
    const alpha = screen.getByRole("article", { name: "Working tree /proj/alpha" });
    expect(within(alpha).getByText("/proj/alpha")).toBeInTheDocument();
    expect(
      within(alpha).getByText("crates/wisphive_daemon/src/state/summaries.rs"),
    ).toBeInTheDocument();
    expect(
      within(alpha).getByText("notes/scratchpad-with-a-quite-long-filename.txt"),
    ).toBeInTheDocument();
  });

  it("attributes each change: agent id where derivable, human/unknown otherwise", () => {
    renderStrip([DIRTY_ALPHA]);
    const alpha = screen.getByRole("article", { name: "Working tree /proj/alpha" });
    expect(within(alpha).getByText("agent: cc-q3-worker")).toBeInTheDocument();
    expect(within(alpha).getByText("human/unknown")).toBeInTheDocument();
  });

  it("renders the FULL generated commit message (header + body), exactly what copy yields", () => {
    renderStrip([DIRTY_ALPHA]);
    const expected = generateCommitMessage(DIRTY_ALPHA);
    expect(expected).not.toBeNull();
    const alpha = screen.getByRole("article", { name: "Working tree /proj/alpha" });
    const pre = alpha.querySelector(".worktree-commit-msg");
    expect(pre?.textContent).toBe(expected!.message);
    // Body reachability: every changed path appears in the rendered message.
    for (const c of DIRTY_ALPHA.changes) {
      expect(pre?.textContent).toContain(c.path);
    }
  });

  it("has ZERO write affordances (spec §5 hard constraint): only copy buttons are interactive", () => {
    renderStrip([DIRTY_ALPHA, DIRTY_BRAVO, tree({ project: "/proj/clean" })]);

    // Enumerate EVERY interactive element in the strip, Board-style.
    const buttons = screen.getAllByRole("button");
    expect(buttons.length).toBeGreaterThan(0);
    for (const button of buttons) {
      expect(button.className).toContain("copy-btn");
      // Every interactive element is a COPY affordance — its accessible name
      // must lead with "Copy", never a git verb (commit/stage/push/…).
      expect(button).toHaveAccessibleName(expect.stringMatching(/^copy/i));
    }
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.queryByRole("link")).toBeNull();
    expect(document.querySelectorAll("input, textarea, select, a, form")).toHaveLength(0);
    // And the read-only contract is stated on the surface.
    expect(screen.getByText(/Read-only mirror — you own git/)).toBeInTheDocument();
  });

  it("renders clean repos, non-git dirs, detached HEAD, and probe errors distinctly", () => {
    renderStrip([
      tree({ project: "/proj/clean" }),
      tree({ project: "/proj/plain", is_git_repo: false, branch: undefined, head: undefined }),
      tree({
        project: "/proj/detached",
        branch: undefined,
        detached: true,
        head: "deadbeefcafe1234",
      }),
      tree({ project: "/proj/broken", is_git_repo: false, error: "git status timed out" }),
    ]);

    const clean = screen.getByRole("article", { name: "Working tree /proj/clean" });
    expect(within(clean).getByText("clean — nothing to commit")).toBeInTheDocument();
    // No commit message block for a clean tree.
    expect(clean.querySelector(".worktree-commit-msg")).toBeNull();

    const plain = screen.getByRole("article", { name: "Working tree /proj/plain" });
    expect(within(plain).getByText("not a git repository")).toBeInTheDocument();

    const detached = screen.getByRole("article", { name: "Working tree /proj/detached" });
    expect(within(detached).getByText("detached @ deadbeefcafe")).toBeInTheDocument();

    const broken = screen.getByRole("article", { name: "Working tree /proj/broken" });
    expect(within(broken).getByText(/probe error: git status timed out/)).toBeInTheDocument();
  });

  it("renders renames with both paths and flags truncated change lists", () => {
    renderStrip([
      tree({
        project: "/proj/renamed",
        changes: [change("README2.md", "R.", { orig_path: "README.md" })],
        changes_truncated: true,
      }),
    ]);
    expect(screen.getByText("README.md → README2.md")).toBeInTheDocument();
    expect(screen.getByText(/More changes exist than shown/)).toBeInTheDocument();
  });

  it("renders the empty state and polls the daemon on mount + interval", () => {
    const onLoad = vi.fn();
    renderStrip([], onLoad);
    expect(
      screen.getByText("No active repositories known to the daemon"),
    ).toBeInTheDocument();
    expect(onLoad).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(15_000);
    expect(onLoad).toHaveBeenCalledTimes(2);
  });

  it("renders agent-controlled path text inertly (no HTML interpretation)", () => {
    const hostile = tree({
      project: "/proj/hostile",
      changes: [change('<img src=x onerror="alert(1)">.rs', "??")],
    });
    renderStrip([hostile]);
    // The literal text renders; no img element is created.
    expect(screen.getByText('<img src=x onerror="alert(1)">.rs')).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });
});
