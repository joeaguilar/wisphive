# Plan: Fleet itr DB Addressing — create/update issues from inside Wisphive

_Last reviewed: 2026-07-05_

Backlog: itr#474 (create issues from inside Wisphive), itr#475 ('Create itr
issue' button on agent-history items), itr#476 (upstream-tracking stub). Upstream
dependency filed in the itr repo as itr#214. Related: itr#448 (fleet-cockpit
spike), epic itr#349 (project discovery + cross-project config sharing).
Decision record: none yet — propose folding into the itr#448 fleet-cockpit ADR.

## Problem

Wisphive is a control plane sitting over many projects at once. An operator
reviewing agent activity in the TUI / web UI cannot turn what they see — a
denial, a risky Bash command, an agent message — into tracked work without
leaving the app and `cd`-ing into the right project. The project already knows
each agent's `cwd`, so Wisphive knows which tracker a given decision belongs to.
It just has no first-class way to write into that tracker.

`itr` is the tracker in use across the fleet, but it resolves its database by
walking up from the current directory. Wisphive is a long-lived process
supervising N projects; it will not `chdir` per call, and per-call `cwd`
juggling in an async multiplexed daemon is a footgun. To create/update/close an
issue on a project's behalf, Wisphive must be able to point `itr` at that
project's `.itr.db` directly.

## Product Boundary

### In Scope

- A "new issue" affordance in the Wisphive UI (TUI and web) that files an `itr`
  issue in the **active project's** tracker.
- Prefill from the surface the operator is looking at (a decision, a history
  item) — title/body derived from that entry's content.
- Availability gating: the affordance is shown only when `itr` is available for
  the active project; hidden otherwise (no dead button).
- Success/failure feedback: surface the created issue ID; show errors, never
  swallow them.

### Out of Scope

- A general project-management UI. Wisphive files and links issues; it does not
  become an `itr` editor.
- Reading/rendering the full backlog inside Wisphive (that is the fleet-cockpit
  spike itr#448, and the linking design in
  [plan-fleet-issue-linking.md](plan-fleet-issue-linking.md)).
- Bundling `itr` into Wisphive. `itr` stays an external binary discovered on
  PATH.

## Upstream vs Wisphive boundary

This design has a hard dependency on an **upstream `itr` change** (itr repo
itr#214, spec `spec-control-plane-db-addressing.md` in that repo). Wisphive
cannot ship itr#474/#475 correctly until it lands, because two current `itr`
behaviors break the control-plane use case:

1. **Directory addressing.** `itr`'s `--db` / `ITR_DB_PATH` override is used
   verbatim as the SQLite *file*. Wisphive knows a project **root**, not the
   `.itr.db` path. Upstream must resolve a directory to `<dir>/.itr.db` so
   Wisphive can address a project by the `cwd` it already tracks.
2. **Precedence.** For every command except `itr init`, an ambient
   `ITR_DB_PATH` **wins over** an explicit `--db` flag. If Wisphive ever runs
   with `ITR_DB_PATH` set (e.g. its own meta-tracker) and passes `--db
   <project>` per call, the write silently lands in the wrong database.
   Upstream must unify precedence to `--db` > `ITR_DB_PATH` > walk-up.

**Wisphive must not work around this** by mutating its own process `cwd` or by
constructing `.itr.db` paths with string concatenation and hoping the precedence
holds — both are the exact silent-wrong-DB failure the upstream change exists to
prevent. Gate the feature on `itr` advertising directory + flag-precedence
support (detect via `itr --version` / a capability probe), and degrade to
"hidden" until then.

## Design

### Availability probe (per project)

A project can create issues iff:
- `itr` is on PATH (probe once, cache), **and**
- the project has an `.itr.db` (either `<root>/.itr.db` exists, or `itr --db
  <root> stats` succeeds), **and**
- the installed `itr` supports directory `--db` + flag-over-env precedence
  (capability gate; see boundary above).

Cache the result per project; re-probe on project rescan (ties into epic
itr#349's discovery/audit core). The UI reads this flag to show/hide the
affordance.

### Invocation model

Per-call binary invocation — no daemon, no long-lived `itr` connection:

```
itr --db <project-root> add "<title>" -k <kind> -p <priority> \
    -c "<body>" -a "<acceptance>" --tags "<...>" -f json
```

- `--db <project-root>` (a directory, post-itr#214) targets the project tracker
  without `chdir`.
- Attribution: set `ITR_AGENT=wisphive` (or `wisphive:<operator>`) in the child
  env so the audit log distinguishes control-plane-authored issues.
- Parse `-f json` output to recover the new issue ID for the success toast.
- Run the child with an explicit, empty-ish env for `ITR_DB_PATH` — **never
  inherit an ambient `ITR_DB_PATH`** into the child, so the explicit `--db`
  is unambiguous even on an older `itr` (belt-and-suspenders over the upstream
  precedence fix).

### Prefill (itr#475)

Each agent-history item and decision row carries enough to seed an issue:
- **title** — a truncated, char-safe summary of the entry (reuse the shared
  char-aware truncate helper; do **not** byte-slice — see itr#362).
- **body** — provenance-first, matching `STORY_STYLE.md`: open with where it came
  from ("From Wisphive history: session `<id>`, project `<path>`, decision
  `<tool>` at `<ts>`"), then the raw (redacted) tool input / result.
- **redaction** — the body must be built from the **already-redacted** surface
  (`wisphive_protocol::redact`, itr#89). Never lift raw secrets out of the live
  in-memory queue into an issue body.

### Surfaces

- **Web**: a button adjacent to the existing copy control on history/decision
  items; a small modal for title/priority/kind confirmation before filing.
  Agent-controlled prefilled text is rendered as React nodes / through the
  audited sanitizer — never `dangerouslySetInnerHTML` (CLAUDE.md web rule).
- **TUI**: a keybinding on the queue/history/session panels (shown in the status
  bar per the project's keybinding-visibility rule); a prompt for
  title/priority.

## Risks & Failure Modes

- **Wrong-DB write** — mitigated by the upstream precedence fix + never
  inheriting `ITR_DB_PATH` into the child. This is the highest-severity failure:
  an issue silently filed into the wrong project's tracker.
- **`itr` absent / older version** — feature hidden, not broken. Capability probe
  is the gate.
- **Secret leak into issue body** — mitigated by building bodies only from the
  redacted projection.
- **`itr add` failure mid-flow** — surface the stderr to the operator; do not
  retry blindly onto a possibly-different DB.

## Acceptance (feature-level)

- With a project whose root has an `.itr.db`, filing from the Wisphive UI creates
  an issue in **that** project's DB (asserted against the DB, not just a success
  toast) and returns its ID to the UI.
- With `ITR_DB_PATH` set in Wisphive's own environment, a filed issue still lands
  in the targeted project, not the ambient DB.
- For a project with no `itr`, the affordance is absent from both TUI and web
  (runtime evidence: screenshot + TUI snapshot).
- A history item containing multi-byte content (`ship 🚀`) prefills and files
  without panic.
- Issue bodies contain no unredacted secrets for a decision whose input carried
  one (redaction regression).

## Sequencing

1. Upstream itr#214 lands (directory `--db` + precedence). **Blocker.**
2. Wisphive availability probe + capability gate (feeds epic itr#349 discovery).
3. itr#474 — new-issue affordance + invocation model + redacted prefill.
4. itr#475 — history-item button reusing #474's plumbing.
5. Fold issue **linking** (not just creation) in via
   [plan-fleet-issue-linking.md](plan-fleet-issue-linking.md).
