#!/usr/bin/env python3
"""itr_snapshot_check.py — keep `.itr/issues.jsonl` in lockstep with `.itr.db`.

`.itr.db` (SQLite, gitignored) is the live issue tracker. `.itr/issues.jsonl`
is its tracked, git-visible form: `itr export` output, one JSON line per issue
bundling the issue row plus its notes, audit events, blockers and relations,
sorted by issue id and byte-deterministic for an unchanged database.

Modes
  (default)   compare `itr export` against the snapshot; exit 1 on drift.
  --write     regenerate the snapshot atomically from `itr export`.
  --strict    exit 1 (instead of 0) when the check has to SKIP.

Exit codes: 0 PASS / SKIP (itr not runnable, no database, no snapshot yet),
1 DRIFT (or SKIP under --strict), 2 could not produce a snapshot in --write.

Why the compare normalizes: `itr import` preserves ISSUE ids (every `itr#NNN`
reference in the docs stays valid) but assigns fresh surrogate row ids to
notes, audit events and relations. A clone that ran `just itr-restore`
therefore exports the same content with different `id` fields on those rows.
The check strips those surrogate ids and sorts the rows before comparing, so
a freshly restored clone is not reported as drift while any real change —
issue fields, note text, event history, blockers, relations — still is.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_DIR = Path(__file__).resolve().parent.parent
SNAPSHOT = REPO_DIR / ".itr" / "issues.jsonl"
SURROGATE_ID_LISTS = ("notes", "events", "relations")


# ── itr access ────────────────────────────────────────────────────────────


def run_export() -> str | None:
    """`itr export` in the repo root, or None when itr cannot run here."""
    try:
        proc = subprocess.run(
            ["itr", "export", "--export-format", "jsonl"],
            cwd=REPO_DIR,
            capture_output=True,
            text=True,
        )
    except FileNotFoundError:
        return None
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        return None
    return proc.stdout


# ── normalization ─────────────────────────────────────────────────────────


def parse(text: str) -> dict[int, dict]:
    """Index issue bundles by issue id. Raises on malformed input."""
    index: dict[int, dict] = {}
    for lineno, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line:
            continue
        rec = json.loads(line)
        issue_id = rec["issue"]["id"]
        if issue_id in index:
            raise ValueError(f"line {lineno}: duplicate issue id {issue_id}")
        index[issue_id] = rec
    return index


def canonical(rec: dict) -> str:
    """Bundle with surrogate row ids removed and row lists order-insensitive."""
    rec = json.loads(json.dumps(rec))  # deep copy
    for key in SURROGATE_ID_LISTS:
        rows = rec.get(key) or []
        for row in rows:
            row.pop("id", None)
        rec[key] = sorted(rows, key=lambda r: json.dumps(r, sort_keys=True))
    rec["blocked_by"] = sorted(rec.get("blocked_by") or [])
    return json.dumps(rec, sort_keys=True)


def diff(committed: dict[int, dict], live: dict[int, dict]):
    added = sorted(set(live) - set(committed))
    removed = sorted(set(committed) - set(live))
    changed = sorted(
        i for i in committed.keys() & live.keys()
        if canonical(committed[i]) != canonical(live[i])
    )
    return added, removed, changed


# ── modes ─────────────────────────────────────────────────────────────────


def write_snapshot() -> int:
    text = run_export()
    if text is None:
        print("ERROR: `itr export` did not run — snapshot left untouched")
        return 2
    try:
        index = parse(text)
    except (ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"ERROR: export is not valid issue JSONL ({exc}) — snapshot left untouched")
        return 2
    if not index:
        print("ERROR: export contained no issues — refusing to write an empty snapshot")
        return 2
    SNAPSHOT.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=SNAPSHOT.parent, prefix=".issues.", suffix=".jsonl.tmp")
    try:
        with os.fdopen(fd, "w") as fh:
            fh.write(text)
        os.replace(tmp, SNAPSHOT)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    rel = SNAPSHOT.relative_to(REPO_DIR)
    print(f"WROTE: {rel} ({len(index)} issues)")
    return 0


def check(strict: bool) -> int:
    rel = SNAPSHOT.relative_to(REPO_DIR)
    skip_rc = 1 if strict else 0
    if not SNAPSHOT.exists():
        print(f"SKIP: {rel} does not exist yet — run `just itr-snapshot`")
        return skip_rc
    text = run_export()
    if text is None:
        print("SKIP: itr not runnable here (not installed, or no .itr.db)")
        return skip_rc
    try:
        live = parse(text)
        committed = parse(SNAPSHOT.read_text())
    except (ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"DRIFT: could not parse snapshot/export ({exc})")
        return 1
    added, removed, changed = diff(committed, live)
    if not (added or removed or changed):
        print(f"PASS: {rel} matches .itr.db ({len(live)} issues)")
        return 0
    print(f"DRIFT: {rel} is stale vs .itr.db — run `just itr-snapshot` and commit it")
    if added:
        print(f"  added in db, missing from snapshot ({len(added)}): {added[:20]}")
    if removed:
        print(f"  in snapshot, missing from db ({len(removed)}): {removed[:20]}")
    if changed:
        print(f"  changed ({len(changed)}): {changed[:20]}")
    return 1


def main(argv: list[str]) -> int:
    args = set(argv[1:])
    unknown = args - {"--write", "--strict"}
    if unknown:
        print(f"usage: {Path(argv[0]).name} [--write] [--strict]  (unknown: {sorted(unknown)})")
        return 2
    if "--write" in args:
        return write_snapshot()
    return check(strict="--strict" in args)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
