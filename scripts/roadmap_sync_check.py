#!/usr/bin/env python3
"""roadmap_sync_check.py — lint the docs/ROADMAP.md <-> itr <-> crate-workspace seam.

Deterministic drift detector (no LLM). Ported from rustglichur's
scripts/roadmap_sync_check.py and adapted to wisphive's repo layout:

  * crates live in crates/<name>/ and are named `wisphive_<thing>` (not `bg-*`)
  * the roadmap is docs/ROADMAP.md with columns:
      | Section | Status | Size | Linked itr | Notes |
    where section IDs look like `§A.1`, links look like `itr#NN` (with an
    optional inline `(closed/open/in-progress)` annotation), and agent-owned
    cells carry `<!-- auto -->` / `<!-- po:override -->` markers.
  * itr runs in-repo (local .itr.db) — no `cd ..` like rustglichur's monorepo.

Drift classes:

  C1  crate-coverage      a crates/<name>/ that no roadmap row references.
                          wisphive's roadmap is section-organized (it does not
                          map crates 1:1 to rows), so when NO row names any
                          crate this is reported as WARN, not ERROR — there is
                          no per-crate convention to violate. If SOME crates
                          are mapped but others are missing, the missing ones
                          are ERROR (the real "a new crate slipped the map" case).
  C2  link-integrity      an itr#ID in a row that is missing from itr, or a
                          row's inline "(closed/open/...)" annotation that
                          contradicts the issue's real itr status.
  C3  epic-hygiene        an OPEN epic whose children are all done (candidate
                          close), an open epic with no formally-parented
                          children (INFO), or an open issue parented to an
                          already-closed epic (WARN).
  C4  orphan-subtask      an open "N.M:"-style subtask with no parent epic.
  C5  bucket-tag          a v2-deferred issue linked from a v1 row.

Severities: error / warn / info. Exit code: 0 if no ERROR findings
(WARN/INFO don't fail the gate); 1 if any ERROR; 2 on harness failure
(itr not runnable, or ROADMAP missing). --strict also fails on WARN.
--json emits findings as JSON.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO_DIR = Path(__file__).resolve().parent.parent
ROADMAP_PATH = REPO_DIR / "docs" / "ROADMAP.md"
CRATES_DIR = REPO_DIR / "crates"

DONE_STATES = {"done", "wontfix"}
OPEN_STATES = {"open", "in-progress"}

# ── itr access ────────────────────────────────────────────────────────────


def itr(*args: str) -> subprocess.CompletedProcess:
    """Run itr in the repo root. wisphive's .itr.db is local, so no cwd hop."""
    return subprocess.run(
        ["itr", *args], cwd=REPO_DIR, capture_output=True, text=True
    )


def load_all_issues() -> dict[int, dict]:
    """Index every issue by id from `itr export` (full DB dump, jsonl).

    `itr list --status` silently EXCLUDES blocked issues, so it under-reports
    (a ✅ row legitimately links a blocked-but-open follow-up). `itr list
    --parent` excludes blocked children too, which would let C3 misjudge an
    epic as "all done". export is the only complete source. Each jsonl line
    bundles one issue under the `issue` key (alongside notes/events/relations).
    """
    proc = itr("export", "--export-format", "jsonl")
    index: dict[int, dict] = {}
    if proc.returncode != 0:
        return index
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        issue = rec.get("issue") if isinstance(rec, dict) else None
        if issue and "id" in issue:
            index[int(issue["id"])] = issue
    return index


def build_children(issues: dict[int, dict]) -> dict[int, list[dict]]:
    """Map epic_id -> [child issue]. Derived in-memory from parent_id so blocked
    children are never dropped (unlike `itr list --parent`)."""
    kids: dict[int, list[dict]] = {}
    for issue in issues.values():
        pid = issue.get("parent_id")
        if pid is not None:
            kids.setdefault(int(pid), []).append(issue)
    return kids


# ── ROADMAP.md parsing ──────────────────────────────────────────────────────

# §A.1, §C.4, §A.1.2 — letter group + numeric subsection(s).
SECTION_ID_RE = re.compile(r"^(§[A-Z]\.\d+(?:\.\d+)?)")
# wisphive crates are named wisphive_<thing> (snake_case dir + Cargo.toml).
CRATE_RE = re.compile(r"\b(wisphive_[a-z0-9]+(?:_[a-z0-9]+)*)\b")
# itr#324 (closed) — id plus an optional inline annotation in parens.
LINK_ANNOT_RE = re.compile(r"itr#(\d+)\s*\(([^)]*)\)")
# any itr#NN or bare #NN reference in the linked cell.
LINK_BARE_RE = re.compile(r"#(\d+)")
SENTINEL_RE = re.compile(r"<!--.*?-->")


@dataclass
class Row:
    section: str
    title: str
    status: str            # ✅ / 🟡 / ❌ / "?"
    bucket: str            # v1 / v2 / excluded
    crate: str | None
    linked: list[int] = field(default_factory=list)
    annotations: dict[int, str] = field(default_factory=dict)  # id -> annotation text
    line_no: int = 0


def _clean(cell: str) -> str:
    """Strip auto/po markers and any other HTML comment, then trim."""
    return SENTINEL_RE.sub("", cell).strip()


def _bucket_for_header(low: str) -> str | None:
    """Classify a `## ...` section header into a v1/v2/excluded bucket.

    Returns None if the header is not a bucket-defining "## Sections ..." /
    "## ... Release" header (so we don't reset bucket on prose headers).
    """
    if low.startswith("## sections"):
        if "v2" in low:
            return "v2"
        if "exclud" in low:
            return "excluded"
        # "## Sections - v1" and "## Sections - Hardening and Release"
        # are both part of the v1 release boundary.
        return "v1"
    return None


def parse_roadmap(path: Path) -> list[Row]:
    rows: list[Row] = []
    bucket = "v1"
    for i, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        low = raw.lower()
        if low.startswith("## "):
            new_bucket = _bucket_for_header(low)
            if new_bucket is not None:
                bucket = new_bucket
            continue
        if not raw.lstrip().startswith("| §"):
            continue
        cells = raw.split("|")
        # leading/trailing empty from the surrounding pipes
        cells = [c for c in cells if c.strip() != ""]
        if len(cells) < 2:
            continue
        section_cell = _clean(cells[0])
        m = SECTION_ID_RE.match(section_cell)
        if not m:
            continue
        section_id = m.group(1)
        title = section_cell[len(section_id):].strip()
        status_cell = _clean(cells[1]) if len(cells) > 1 else "?"
        status = next((s for s in ("✅", "🟡", "❌") if s in status_cell), "?")
        # Columns: 0=Section 1=Status 2=Size 3=Linked itr 4=Notes
        linked_cell = cells[3] if len(cells) > 3 else ""
        notes_cell = cells[4] if len(cells) > 4 else ""
        # Crate may be named anywhere in the row (section title or notes).
        crate_m = CRATE_RE.search(section_cell) or CRATE_RE.search(notes_cell)
        row = Row(
            section=section_id,
            title=title,
            status=status,
            bucket=bucket,
            crate=crate_m.group(1) if crate_m else None,
            line_no=i,
        )
        for mid, annot in LINK_ANNOT_RE.findall(linked_cell):
            row.annotations[int(mid)] = annot.strip()
        row.linked = sorted({int(x) for x in LINK_BARE_RE.findall(linked_cell)})
        rows.append(row)
    return rows


def workspace_crates(crates_dir: Path) -> set[str]:
    if not crates_dir.is_dir():
        return set()
    return {
        p.name
        for p in crates_dir.iterdir()
        if p.is_dir() and (p / "Cargo.toml").is_file()
    }


# ── findings ────────────────────────────────────────────────────────────────


@dataclass
class Finding:
    check: str
    severity: str   # error / warn / info
    subject: str
    message: str
    fix_hint: str = ""


def annotation_implies(annot: str) -> str | None:
    """Map a roadmap inline annotation to an expected itr state class."""
    a = annot.lower()
    if "in-progress" in a or "in progress" in a:
        return "in-progress"
    if "closed" in a or "done" in a:
        return "done"
    if "open" in a:
        return "open"
    return None


def run_checks(rows: list[Row], issues: dict[int, dict],
               children: dict[int, list[dict]], crates: set[str]) -> list[Finding]:
    out: list[Finding] = []

    # C1 — crate coverage.
    # wisphive's roadmap is section-organized, not crate-organized. If no row
    # names a crate, there is no per-crate mapping convention to violate, so we
    # report the gap once as WARN. If SOME crates are mapped, treat any missing
    # ones as ERROR (a real "new crate slipped the roadmap" regression).
    mapped = {r.crate for r in rows if r.crate}
    missing = sorted(crates - mapped)
    if mapped:
        for crate in missing:
            out.append(Finding(
                "crate-coverage", "error", crate,
                f"workspace crate `{crate}` has no roadmap row",
                "add a row under the appropriate §group (status from itr), "
                "or name the crate in an existing row's Notes",
            ))
    elif crates:
        out.append(Finding(
            "crate-coverage", "warn", "crates/",
            f"roadmap maps 0 of {len(crates)} workspace crates by name "
            f"({', '.join(missing)})",
            "the roadmap is section-organized; if you want per-crate coverage "
            "tracking, name each crate in its section row's Notes",
        ))

    # C2 — link integrity + annotation vs. actual status
    for r in rows:
        for mid in r.linked:
            issue = issues.get(mid)
            if issue is None:
                out.append(Finding(
                    "link-integrity", "error", f"#{mid}",
                    f"{r.section} links itr#{mid}, which does not exist in itr",
                    "remove the stale link or correct the ID",
                ))
                continue
            annot = r.annotations.get(mid)
            if not annot:
                continue
            implied = annotation_implies(annot)
            actual = issue["status"]
            if implied == "done" and actual not in DONE_STATES:
                out.append(Finding(
                    "link-integrity", "error", f"#{mid}",
                    f'{r.section} annotates itr#{mid} as "{annot}" but itr status is {actual}',
                    "the work is not actually closed — fix the annotation or finish/close the issue",
                ))
            elif implied == "open" and actual in DONE_STATES:
                out.append(Finding(
                    "link-integrity", "warn", f"#{mid}",
                    f'{r.section} annotates itr#{mid} as "{annot}" but itr status is {actual} — row may need a status bump',
                    "refresh the cell (likely a sprint closed this); run /roadmap --update",
                ))
            elif implied == "in-progress" and actual != "in-progress":
                out.append(Finding(
                    "link-integrity", "warn", f"#{mid}",
                    f'{r.section} annotates itr#{mid} as "{annot}" but itr status is {actual}',
                    "refresh the annotation",
                ))

    # C3 — epic hygiene (open epics): all children done -> candidate close
    for iid, issue in sorted(issues.items()):
        if issue.get("kind") != "epic" or issue.get("status") not in OPEN_STATES:
            continue
        kids = children.get(iid, [])
        if kids and all(k["status"] in DONE_STATES for k in kids):
            out.append(Finding(
                "epic-hygiene", "error", f"#{iid}",
                f'open epic #{iid} "{issue["title"][:50]}" has {len(kids)} children, all done',
                "close the epic (all child work delivered)",
            ))
        elif not kids:
            out.append(Finding(
                "epic-hygiene", "info", f"#{iid}",
                f'open epic #{iid} "{issue["title"][:50]}" has 0 formally-parented children',
                "either a tracking shell to close, or its subtasks need parent_id set (see C4)",
            ))

    # C3b — open issue whose parent epic is already closed
    for iid, issue in sorted(issues.items()):
        if issue.get("status") not in OPEN_STATES:
            continue
        pid = issue.get("parent_id")
        if pid is None:
            continue
        parent = issues.get(int(pid))
        if parent and parent.get("status") in DONE_STATES:
            out.append(Finding(
                "epic-hygiene", "warn", f"#{iid}",
                f'open issue #{iid} is parented to #{pid} which is {parent["status"]}',
                "reparent to a live epic or close it — open residual under a closed parent goes stale",
            ))

    # C4 — orphan subtasks (heuristic: "N.M:" or "N.M.x:" title prefix, open, no parent)
    subtask_re = re.compile(r"^\d+\.\d+")
    for iid, issue in sorted(issues.items()):
        if issue.get("status") not in OPEN_STATES:
            continue
        if not subtask_re.match(issue.get("title", "")):
            continue
        if not issue.get("parent_id"):
            out.append(Finding(
                "orphan-subtask", "warn", f"#{iid}",
                f'#{iid} "{issue["title"][:50]}" looks like a subtask but has no parent epic',
                "itr update <id> --parent <epic> so epic-hygiene (C3) can track it",
            ))

    # C5 — bucket/tag consistency (v2-deferred linked from a v1 row)
    for r in rows:
        if r.bucket != "v1":
            continue
        for mid in r.linked:
            issue = issues.get(mid)
            if not issue:
                continue
            tags = issue.get("tags") or []
            if isinstance(tags, str):
                tags = [t.strip() for t in tags.split(",")]
            if "v2-deferred" in tags:
                out.append(Finding(
                    "bucket-tag", "warn", f"#{mid}",
                    f"{r.section} (v1) links itr#{mid}, which is tagged v2-deferred",
                    "move the work to a v2 row, or drop the v2-deferred tag if it's actually v1",
                ))

    return out


# ── reporting ────────────────────────────────────────────────────────────────

SEV_ORDER = {"error": 0, "warn": 1, "info": 2}
SEV_LABEL = {"error": "ERROR", "warn": "WARN ", "info": "INFO "}


def main() -> int:
    ap = argparse.ArgumentParser(description="Lint the ROADMAP.md <-> itr <-> crates seam.")
    ap.add_argument("--json", action="store_true", help="emit findings as JSON")
    ap.add_argument("--strict", action="store_true", help="exit nonzero on WARN too")
    args = ap.parse_args()

    if itr("stats").returncode != 0:
        print("roadmap-sync: `itr` not runnable from the repo root", file=sys.stderr)
        return 2
    if not ROADMAP_PATH.is_file():
        print(f"roadmap-sync: {ROADMAP_PATH} not found", file=sys.stderr)
        return 2

    rows = parse_roadmap(ROADMAP_PATH)
    issues = load_all_issues()
    children = build_children(issues)
    crates = workspace_crates(CRATES_DIR)
    findings = run_checks(rows, issues, children, crates)
    findings.sort(key=lambda f: (SEV_ORDER[f.severity], f.check, f.subject))

    errors = sum(1 for f in findings if f.severity == "error")
    warns = sum(1 for f in findings if f.severity == "warn")
    infos = sum(1 for f in findings if f.severity == "info")

    if args.json:
        print(json.dumps({
            "summary": {"rows": len(rows), "issues_indexed": len(issues),
                        "crates": len(crates), "error": errors, "warn": warns, "info": infos},
            "findings": [f.__dict__ for f in findings],
        }, indent=2, ensure_ascii=False))
    else:
        print(f"roadmap-sync — {len(rows)} rows · {len(crates)} crates · {len(issues)} issues indexed")
        if not findings:
            print("  ✓ no drift detected — roadmap, itr, and workspace are in sync")
        for f in findings:
            print(f"  {SEV_LABEL[f.severity]} [{f.check}] {f.subject}: {f.message}")
            if f.fix_hint:
                print(f"        ↳ {f.fix_hint}")
        print(f"\n  {errors} error · {warns} warn · {infos} info")

    if errors:
        return 1
    if args.strict and warns:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
