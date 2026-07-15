// Deterministic Conventional Commits v1.0.0 message generation for the
// working-tree strip (itr#401, spec §5.3). No LLM, no randomness: the message
// is a pure function of the probed tree state + attribution, regenerated
// whenever the tree changes (each poll re-derives it).
//
// ── The heuristic (documented per the story AC) ──────────────────────────────
//
// type:
//   1. every change is a test file            → `test`
//   2. every change is a docs file            → `docs`
//   3. any ADDED/untracked non-test/non-docs  → `feat`   (new capability shape)
//   4. every change is a deletion             → `chore`  (removal shape)
//   5. otherwise (modifications/renames only) → `refactor`
//      `fix` vs `refactor` is NOT derivable from change shape alone — a
//      deterministic generator must pick one, and `refactor` is the honest
//      default for "tracked files were modified"; the human retypes it to
//      `fix` when the work was a bug fix.
//
// scope: the dominant directory — `crates/<name>` counts as `<name>`, else the
//   first path segment. Chosen only when a strict majority (> half) of changes
//   share it; otherwise the scope is omitted. Sanitized to a Conventional
//   Commits-safe token.
//
// summary: imperative verb (by type) + changed file basenames, degrading to
//   "<verb> N files" so the FULL header is ≤ 72 chars by construction.
//
// body: the full untruncated change list (no-truncation rule: this is the only
//   place the full path set + per-change attribution renders into the copyable
//   message) with per-change agent attribution from the decision audit stream,
//   plus an Attribution summary line.

import type { WorktreeChange, WorktreeStatus } from "../types/protocol";

export const COMMIT_TYPES = [
  "feat",
  "fix",
  "docs",
  "style",
  "refactor",
  "perf",
  "test",
  "build",
  "ci",
  "chore",
] as const;

export type CommitType = (typeof COMMIT_TYPES)[number];

/** Conventional Commits v1.0.0 header: `type(scope)!?: description`. */
export const CONVENTIONAL_HEADER_RE = new RegExp(
  `^(${COMMIT_TYPES.join("|")})(\\([^()\\r\\n]+\\))?!?: \\S.*$`,
);

export const MAX_HEADER_LENGTH = 72;

export function isConventionalCommitHeader(header: string): boolean {
  return CONVENTIONAL_HEADER_RE.test(header) && header.length <= MAX_HEADER_LENGTH;
}

export interface GeneratedCommit {
  header: string;
  body: string;
  /** header + blank line + body — the full copyable message. */
  message: string;
  type: CommitType;
  scope: string | null;
}

const TEST_DIR_RE = /(^|\/)(tests?|__tests__)\//;
const TEST_FILE_RE = /(\.(test|spec)\.[^./]+|_test\.[^./]+)$|(^|\/)tests\.rs$/;
const DOCS_DIR_RE = /(^|\/)docs?\//;
const DOCS_FILE_RE = /\.(md|mdx|rst|adoc|txt)$/i;

function isTestPath(path: string): boolean {
  return TEST_DIR_RE.test(path) || TEST_FILE_RE.test(path);
}

function isDocsPath(path: string): boolean {
  return DOCS_DIR_RE.test(path) || DOCS_FILE_RE.test(path);
}

function isAdded(change: WorktreeChange): boolean {
  return change.status === "??" || change.status.includes("A");
}

function isDeleted(change: WorktreeChange): boolean {
  return change.status.includes("D");
}

function deriveType(changes: WorktreeChange[]): CommitType {
  if (changes.length === 0) return "chore";
  if (changes.every((c) => isTestPath(c.path))) return "test";
  if (changes.every((c) => isDocsPath(c.path))) return "docs";
  if (changes.some((c) => isAdded(c) && !isTestPath(c.path) && !isDocsPath(c.path))) {
    return "feat";
  }
  if (changes.every(isDeleted)) return "chore";
  return "refactor";
}

/** Scope token for one path: `crates/<name>/…` → `<name>`, else the first
 * directory segment; files at the repo root get no scope vote. */
function scopeSegment(path: string): string | null {
  const segments = path.split("/");
  if (segments.length < 2) return null;
  if ((segments[0] === "crates" || segments[0] === "packages") && segments.length >= 3) {
    return segments[1];
  }
  return segments[0];
}

function sanitizeScope(scope: string): string {
  const cleaned = scope.replace(/[^A-Za-z0-9_.-]+/g, "-").replace(/^-+|-+$/g, "");
  return cleaned.length > 0 ? cleaned : "";
}

function deriveScope(changes: WorktreeChange[]): string | null {
  const counts = new Map<string, number>();
  for (const change of changes) {
    const seg = scopeSegment(change.path);
    if (seg === null) continue;
    counts.set(seg, (counts.get(seg) ?? 0) + 1);
  }
  for (const [seg, count] of counts) {
    if (count * 2 > changes.length) {
      const sanitized = sanitizeScope(seg);
      // Keep the header comfortably under budget even for pathological names.
      if (sanitized.length > 0 && sanitized.length <= 24) return sanitized;
    }
  }
  return null;
}

const VERB_BY_TYPE: Record<CommitType, string> = {
  feat: "add",
  fix: "fix",
  docs: "update",
  style: "restyle",
  refactor: "update",
  perf: "tune",
  test: "update",
  build: "update",
  ci: "update",
  chore: "remove",
};

function basename(path: string): string {
  const segments = path.split("/");
  return segments[segments.length - 1] || path;
}

function deriveSummary(type: CommitType, scope: string | null, changes: WorktreeChange[]): string {
  const verb = VERB_BY_TYPE[type];
  const prefixLen = type.length + (scope ? scope.length + 2 : 0) + 2; // "type(scope): "
  const budget = MAX_HEADER_LENGTH - prefixLen;

  const names: string[] = [];
  for (const change of changes) {
    const name = basename(change.path);
    if (!names.includes(name)) names.push(name);
  }

  const candidates = [
    `${verb} ${names.join(", ")}`,
    names.length > 1 ? `${verb} ${names[0]} and ${names.length - 1} more` : null,
    `${verb} ${changes.length} ${changes.length === 1 ? "file" : "files"}`,
  ].filter((c): c is string => c !== null);

  for (const candidate of candidates) {
    // Headers must be single-line; newlines can't appear (basenames can't
    // contain "/" or "\n" after the split, but be defensive).
    const flat = candidate.replace(/\s+/g, " ").trim();
    if (flat.length > 0 && flat.length <= budget) return flat;
  }
  // Guaranteed-fit fallback (numbers keep this far under any sane budget).
  return `${verb} ${changes.length} files`;
}

function changeLine(change: WorktreeChange): string {
  const rename = change.orig_path ? ` (from ${change.orig_path})` : "";
  const who = change.attributed_to
    ? `agent ${change.attributed_to}${change.attributed_tool ? ` via ${change.attributed_tool}` : ""}`
    : "human/unknown";
  return `- ${change.status} ${change.path}${rename} — ${who}`;
}

function attributionSummary(changes: WorktreeChange[]): string {
  const agents = new Map<string, number>();
  let human = 0;
  for (const change of changes) {
    if (change.attributed_to) {
      agents.set(change.attributed_to, (agents.get(change.attributed_to) ?? 0) + 1);
    } else {
      human += 1;
    }
  }
  const parts: string[] = [];
  const agentTotal = changes.length - human;
  if (agentTotal > 0) {
    const ids = [...agents.keys()].join(", ");
    parts.push(`${agentTotal} agent-made (${ids})`);
  }
  if (human > 0) parts.push(`${human} human/unknown`);
  return `Attribution: ${parts.join(", ")}.`;
}

/**
 * Generate a Conventional Commits message for a dirty tree. Returns null for
 * clean trees / non-repos (nothing to commit). The header is guaranteed to
 * satisfy {@link isConventionalCommitHeader}; the body lists every change
 * (full untruncated paths) with per-change attribution.
 */
export function generateCommitMessage(wt: WorktreeStatus): GeneratedCommit | null {
  if (!wt.is_git_repo || wt.changes.length === 0) return null;

  const type = deriveType(wt.changes);
  const scope = deriveScope(wt.changes);
  const summary = deriveSummary(type, scope, wt.changes);
  const header = `${type}${scope ? `(${scope})` : ""}: ${summary}`;

  const bodyLines = ["Changes:", ...wt.changes.map(changeLine)];
  if (wt.changes_truncated) {
    bodyLines.push("- … more changes not shown (tree exceeds the probe cap)");
  }
  bodyLines.push("", attributionSummary(wt.changes));
  const body = bodyLines.join("\n");

  return { header, body, message: `${header}\n\n${body}`, type, scope };
}
