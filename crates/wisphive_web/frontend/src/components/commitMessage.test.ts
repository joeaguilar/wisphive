import { describe, expect, it } from "vitest";
import {
  generateCommitMessage,
  isConventionalCommitHeader,
  MAX_HEADER_LENGTH,
} from "./commitMessage";
import type { WorktreeChange, WorktreeStatus } from "../types/protocol";

function wt(changes: WorktreeChange[], overrides: Partial<WorktreeStatus> = {}): WorktreeStatus {
  return {
    project: "/proj/alpha",
    is_git_repo: true,
    branch: "main",
    detached: false,
    head: "abc123",
    changes,
    changes_truncated: false,
    probed_at: "2026-07-15T12:00:00Z",
    ...overrides,
  };
}

function change(path: string, status = ".M", overrides: Partial<WorktreeChange> = {}): WorktreeChange {
  return { path, status, ...overrides };
}

describe("generateCommitMessage — type heuristic (documented in commitMessage.ts)", () => {
  it("returns null for clean trees and non-repos (nothing to commit)", () => {
    expect(generateCommitMessage(wt([]))).toBeNull();
    expect(generateCommitMessage(wt([change("a.rs")], { is_git_repo: false }))).toBeNull();
  });

  it("tests-only → test", () => {
    const gen = generateCommitMessage(
      wt([change("crates/x/tests/probe.rs"), change("src/foo.test.ts")]),
    );
    expect(gen?.type).toBe("test");
    expect(gen?.header.startsWith("test")).toBe(true);
  });

  it("docs-only → docs", () => {
    const gen = generateCommitMessage(wt([change("docs/guide.md"), change("README.md")]));
    expect(gen?.type).toBe("docs");
  });

  it("new source files → feat", () => {
    const gen = generateCommitMessage(
      wt([change("src/new_mod.rs", "A."), change("src/lib.rs", ".M")]),
    );
    expect(gen?.type).toBe("feat");
  });

  it("untracked source files count as added → feat", () => {
    const gen = generateCommitMessage(wt([change("src/widget.ts", "??")]));
    expect(gen?.type).toBe("feat");
  });

  it("deletions-only → chore", () => {
    const gen = generateCommitMessage(wt([change("src/old.rs", "D.")]));
    expect(gen?.type).toBe("chore");
  });

  it("modifications-only → refactor (fix is not shape-derivable; deterministic default)", () => {
    const gen = generateCommitMessage(wt([change("src/lib.rs", ".M"), change("src/b.rs", "M.")]));
    expect(gen?.type).toBe("refactor");
  });
});

describe("generateCommitMessage — scope heuristic", () => {
  it("uses the crate name for a crates/<name> majority", () => {
    const gen = generateCommitMessage(
      wt([
        change("crates/wisphive_web/src/a.rs"),
        change("crates/wisphive_web/src/b.rs"),
        change("README.md"),
      ]),
    );
    expect(gen?.scope).toBe("wisphive_web");
    expect(gen?.header).toContain("(wisphive_web):");
  });

  it("uses the dominant top-level dir", () => {
    const gen = generateCommitMessage(wt([change("src/a.rs"), change("src/b.rs")]));
    expect(gen?.scope).toBe("src");
  });

  it("omits the scope when no strict majority exists", () => {
    const gen = generateCommitMessage(wt([change("src/a.rs"), change("docs/b.md")]));
    expect(gen?.scope).toBeNull();
    expect(gen?.header).toMatch(/^[a-z]+: /);
  });
});

describe("generateCommitMessage — output contract", () => {
  it("header is valid Conventional Commits and ≤ 72 chars", () => {
    const gen = generateCommitMessage(wt([change("src/lib.rs")]));
    expect(gen).not.toBeNull();
    expect(isConventionalCommitHeader(gen!.header)).toBe(true);
  });

  it("body lists every change with its FULL path (no-truncation rule) and attribution", () => {
    const changes = [
      change("crates/wisphive_daemon/src/state/summaries.rs", ".M", {
        attributed_to: "cc-q3-worker",
        attributed_tool: "Edit",
      }),
      change("a/very/deeply/nested/directory/structure/file.rs", "??"),
    ];
    const gen = generateCommitMessage(wt(changes));
    expect(gen).not.toBeNull();
    for (const c of changes) {
      expect(gen!.body).toContain(c.path);
    }
    expect(gen!.body).toContain("agent cc-q3-worker via Edit");
    expect(gen!.body).toContain("human/unknown");
    expect(gen!.body).toContain("Attribution: 1 agent-made (cc-q3-worker), 1 human/unknown.");
    expect(gen!.message).toBe(`${gen!.header}\n\n${gen!.body}`);
  });

  it("renames carry the original path in the body", () => {
    const gen = generateCommitMessage(
      wt([change("README2.md", "R.", { orig_path: "README.md" })]),
    );
    expect(gen!.body).toContain("README2.md (from README.md)");
  });

  it("notes the probe cap when changes were truncated", () => {
    const gen = generateCommitMessage(wt([change("src/a.rs")], { changes_truncated: true }));
    expect(gen!.body).toContain("more changes not shown");
  });

  it("degrades to '<first> and N more' when the full file list blows the budget", () => {
    const changes = Array.from({ length: 30 }, (_, i) =>
      change(`src/some_quite_long_module_name_${i}.rs`),
    );
    const gen = generateCommitMessage(wt(changes));
    expect(isConventionalCommitHeader(gen!.header)).toBe(true);
    expect(gen!.header).toContain("and 29 more");
    // The full list still lives in the body.
    for (const c of changes) expect(gen!.body).toContain(c.path);
  });

  it("degrades to a bare count when even one basename blows the budget", () => {
    const huge = `src/${"extremely_long_module_segment_".repeat(4)}impl.rs`;
    const changes = [change(huge), change(`${huge}.bak`)];
    const gen = generateCommitMessage(wt(changes));
    expect(isConventionalCommitHeader(gen!.header)).toBe(true);
    expect(gen!.header).toContain("2 files");
    for (const c of changes) expect(gen!.body).toContain(c.path);
  });
});

// ── Property: ANY tree state yields a valid Conventional Commits header ─────
// Deterministic seeded PRNG (mulberry32) — no new deps, reproducible failures.
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

describe("generateCommitMessage — property: always valid Conventional Commits", () => {
  const SEGMENTS = [
    "src",
    "crates",
    "wisphive_web",
    "docs",
    "tests",
    "__tests__",
    "a",
    "deeply nested dir",
    "weird()chars",
    "ünï-code",
    "x".repeat(80),
  ];
  const EXTENSIONS = [".rs", ".ts", ".test.ts", ".md", ".txt", ".spec.tsx", "", ".tar.gz"];
  const STATUSES = [".M", "M.", "MM", "A.", ".A", "D.", ".D", "??", "R.", "UU"];
  const AGENTS = [undefined, "cc-worker-1", "codex-x", "agent with spaces"];

  it("holds across 500 randomized tree states (seeded)", () => {
    const rand = mulberry32(0x401);
    const pick = <T,>(arr: readonly T[]): T => arr[Math.floor(rand() * arr.length)];

    for (let iter = 0; iter < 500; iter++) {
      const changeCount = 1 + Math.floor(rand() * 40);
      const changes: WorktreeChange[] = Array.from({ length: changeCount }, (_, i) => {
        const depth = Math.floor(rand() * 4);
        const dirs = Array.from({ length: depth }, () => pick(SEGMENTS));
        const name = `${pick(SEGMENTS)}${i}${pick(EXTENSIONS)}`;
        const attributed = pick(AGENTS);
        return {
          path: [...dirs, name].join("/"),
          status: pick(STATUSES),
          attributed_to: attributed,
          attributed_tool: attributed ? pick(["Edit", "Write", "Bash"]) : undefined,
        };
      });
      const gen = generateCommitMessage(wt(changes, { changes_truncated: rand() < 0.1 }));
      expect(gen).not.toBeNull();
      const { header, body } = gen!;
      expect(
        isConventionalCommitHeader(header),
        `iter ${iter}: invalid header ${JSON.stringify(header)}`,
      ).toBe(true);
      expect(header.length).toBeLessThanOrEqual(MAX_HEADER_LENGTH);
      expect(header).not.toContain("\n");
      // Full untruncated reachability: every changed path appears in the body.
      for (const c of changes) {
        expect(body, `iter ${iter}: body missing path ${c.path}`).toContain(c.path);
      }
    }
  });
});
