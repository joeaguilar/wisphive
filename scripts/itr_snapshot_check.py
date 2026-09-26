#!/usr/bin/env python3
"""itr_snapshot_check.py — keep `.itr/issues.jsonl` in lockstep with the itr database.

`.itr.db` (SQLite, gitignored) is the live issue tracker. `.itr/issues.jsonl`
is its tracked, git-visible form: `itr export` output, one JSON line per issue
bundling the issue row plus its notes, audit events, blockers and relations,
sorted by issue id and byte-deterministic for an unchanged database.

Modes
  (default)   compare `itr export` against the snapshot; exit 1 on drift.
  --write     regenerate the snapshot atomically from `itr export`.
  --restore   rebuild the database from the snapshot (fresh-clone bootstrap).
              Imports into whatever database itr resolves (walk-up from the
              repo root, or ITR_DB_PATH); runs `itr init` first when none
              exists. Existing issues whose ids collide are REPLACED by the
              snapshot: this is a pull, not a merge.
  --strict    exit 1 (instead of 0) when the check has to SKIP.
  --root DIR  repo root holding `.itr/issues.jsonl` (default: the git
              toplevel of this script's location, else its parent's parent).

Exit codes: 0 PASS / SKIP (itr not runnable, no database, no snapshot yet),
1 DRIFT (or SKIP under --strict), 2 could not produce or restore a snapshot.

Why the compare normalizes: `itr import` preserves ISSUE ids (every `itr#NNN`
reference in the docs stays valid) but assigns fresh surrogate row ids to
notes, audit events and relations. A clone that restored from the snapshot
therefore exports the same content with different `id` fields on those rows.
The check strips those surrogate ids and sorts the rows before comparing, so
a freshly restored clone is not reported as drift while any real change —
issue fields, note text, event history, blockers, relations — still is.

Second normalization: `itr import` drops blocker edges whose blocker issue is
done/wontfix ("a resolved issue no longer blocks anything, as on close"), the
same pruning `itr close` performs. A database that still carries such stale
edges exports them, but a clone restored from that export does not. The check
therefore ignores a `blocked_by` entry whenever the blocker is resolved on that
side; an edge from an open blocker is always compared.

Requires itr >= 3.3.1 (two-pass import that restores forward references,
events and relations). Older importers abort on forward references.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

SURROGATE_ID_LISTS = ("notes", "events", "relations")


# ── repo root ─────────────────────────────────────────────────────────────


def default_root() -> Path:
    here = Path(__file__).resolve().parent
    try:
        proc = subprocess.run(
            ["git", "-C", str(here), "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
        )
        if proc.returncode == 0 and proc.stdout.strip():
            return Path(proc.stdout.strip())
    except FileNotFoundError:
        pass
    return here.parent


# ── itr access ────────────────────────────────────────────────────────────


def itr(root: Path, *args: str) -> subprocess.CompletedProcess | None:
    """Run itr in the repo root (walk-up + ITR_DB_PATH apply); None if absent."""
    try:
        return subprocess.run(["itr", *args], cwd=root, capture_output=True, text=True)
    except FileNotFoundError:
        return None


def run_export(root: Path) -> str | None:
    proc = itr(root, "export", "--export-format", "jsonl")
    if proc is None:
        return None
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        return None
    return proc.stdout


def db_available(root: Path) -> bool:
    proc = itr(root, "stats", "-q")
    return proc is not None and proc.returncode == 0


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


RESOLVED_STATUSES = {"done", "wontfix"}


def resolved_ids(index: dict[int, dict]) -> set[int]:
    """Issue ids whose blocker edges itr prunes on close and on import."""
    return {
        i for i, rec in index.items()
        if rec["issue"].get("status") in RESOLVED_STATUSES
    }


def canonical(rec: dict, resolved: set[int] = frozenset()) -> str:
    """Bundle with surrogate row ids removed, row lists order-insensitive, and
    blocker edges from resolved issues dropped (itr prunes those on import)."""
    rec = json.loads(json.dumps(rec))  # deep copy
    for key in SURROGATE_ID_LISTS:
        rows = rec.get(key) or []
        for row in rows:
            row.pop("id", None)
        rec[key] = sorted(rows, key=lambda r: json.dumps(r, sort_keys=True))
    rec["blocked_by"] = sorted(
        b for b in (rec.get("blocked_by") or []) if b not in resolved
    )
    return json.dumps(rec, sort_keys=True)


def diff(committed: dict[int, dict], live: dict[int, dict]):
    added = sorted(set(live) - set(committed))
    removed = sorted(set(committed) - set(live))
    res_c, res_l = resolved_ids(committed), resolved_ids(live)
    changed = sorted(
        i for i in committed.keys() & live.keys()
        if canonical(committed[i], res_c) != canonical(live[i], res_l)
    )
    return added, removed, changed


# ── modes ─────────────────────────────────────────────────────────────────


def write_snapshot(root: Path, snapshot: Path) -> int:
    rel = snapshot.relative_to(root)
    text = run_export(root)
    if text is None:
        print(f"ERROR: `itr export` did not run — {rel} left untouched")
        return 2
    try:
        index = parse(text)
    except (ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"ERROR: export is not valid issue JSONL ({exc}) — {rel} left untouched")
        return 2
    if not index:
        print(f"ERROR: export contained no issues — refusing to write an empty {rel}")
        return 2
    snapshot.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=snapshot.parent, prefix=".issues.", suffix=".jsonl.tmp")
    try:
        with os.fdopen(fd, "w") as fh:
            fh.write(text)
        os.replace(tmp, snapshot)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    print(f"WROTE: {rel} ({len(index)} issues)")
    return 0


def restore(root: Path, snapshot: Path) -> int:
    rel = snapshot.relative_to(root)
    if not snapshot.exists():
        print(f"ERROR: {rel} does not exist — nothing to restore from")
        return 2
    if itr(root, "--version") is None:
        print("ERROR: itr is not installed")
        return 2
    if not db_available(root):
        override = os.environ.get("ITR_DB_PATH")
        init_args = ["--db", override, "init"] if override else ["init"]
        proc = itr(root, *init_args)
        if proc is None or proc.returncode != 0:
            print(f"ERROR: `itr {' '.join(init_args)}` failed:\n{(proc.stderr if proc else '').strip()}")
            return 2
        print((proc.stdout or "").strip() or "INIT: created a new database")
    proc = itr(root, "import", "--file", str(snapshot))
    if proc is None or proc.returncode != 0:
        print(f"ERROR: `itr import` failed:\n{(proc.stderr if proc else '').strip()}")
        return 2
    for stream in (proc.stderr, proc.stdout):
        for line in (stream or "").splitlines():
            if line.startswith(("IMPORT:", "REVIEW:")):
                print(line)
    rc = check(root, snapshot, strict=True)
    if rc != 0:
        print(
            "NOTE: --restore replaces issues whose ids are in the snapshot and never "
            "deletes local-only issues, so the database still differs. Export them "
            "with --write (so they reach the snapshot) or remove them by hand."
        )
    return rc


def check(root: Path, snapshot: Path, strict: bool) -> int:
    rel = snapshot.relative_to(root)
    skip_rc = 1 if strict else 0
    if not snapshot.exists():
        print(f"SKIP: {rel} does not exist yet — run the --write mode")
        return skip_rc
    text = run_export(root)
    if text is None:
        print("SKIP: itr not runnable here (not installed, or no database found)")
        return skip_rc
    try:
        live = parse(text)
        committed = parse(snapshot.read_text())
    except (ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"DRIFT: could not parse snapshot/export ({exc})")
        return 1
    added, removed, changed = diff(committed, live)
    if not (added or removed or changed):
        print(f"PASS: {rel} matches the itr database ({len(live)} issues)")
        return 0
    print(f"DRIFT: {rel} is stale vs the itr database — regenerate it (--write) and commit it")
    if added:
        print(f"  added in db, missing from snapshot ({len(added)}): {added[:20]}")
    if removed:
        print(f"  in snapshot, missing from db ({len(removed)}): {removed[:20]}")
    if changed:
        print(f"  changed ({len(changed)}): {changed[:20]}")
    return 1


def main(argv: list[str]) -> int:
    args = argv[1:]
    root: Path | None = None
    flags: set[str] = set()
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--root":
            if i + 1 >= len(args):
                print("usage: --root needs a directory")
                return 2
            root = Path(args[i + 1]).resolve()
            i += 2
            continue
        if a.startswith("--root="):
            root = Path(a.split("=", 1)[1]).resolve()
        elif a in {"--write", "--restore", "--strict"}:
            flags.add(a)
        else:
            print(f"usage: {Path(argv[0]).name} [--write | --restore] [--strict] [--root DIR]  (unknown: {a})")
            return 2
        i += 1
    if "--write" in flags and "--restore" in flags:
        print("usage: --write and --restore are mutually exclusive")
        return 2
    root = root or default_root()
    snapshot = root / ".itr" / "issues.jsonl"
    if "--write" in flags:
        return write_snapshot(root, snapshot)
    if "--restore" in flags:
        return restore(root, snapshot)
    return check(root, snapshot, strict="--strict" in flags)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
