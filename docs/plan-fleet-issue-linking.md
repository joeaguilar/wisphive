# Plan: Fleet Issue Linking — cross-database itr links from the control plane

_Last reviewed: 2026-07-05_

Backlog: none filed yet — this plan proposes the workstream. Related: itr#474/#475
(create issues from inside Wisphive, see
[plan-fleet-itr-db-addressing.md](plan-fleet-itr-db-addressing.md)), itr#476 /
upstream itr#214 (DB addressing), itr#448 (fleet-cockpit spike), epic itr#349
(project discovery + cross-project config sharing). Decision record: none yet —
propose an ADR once the tier boundary is chosen, since "cross-DB links are
informational, never blocking" is a durable, non-obvious constraint.

## Problem

Wisphive supervises many projects, each with its own **independent `itr`
SQLite database**. Work in one project routinely relates to another: a Wisphive
feature (`itr#474`) depends on an upstream `itr` change (filed as `itr#214` in
the *itr* repo). Today that relationship exists only as prose — `itr#NNN`
mentioned in a body — because `itr relate` / `itr depend` cannot cross a
database boundary. The control plane is the one component that knows where every
project's DB lives, so it is the natural place to make cross-project links
real and navigable.

The caveat we keep hitting: **"the cross-repo link is by reference only — the two
trackers are separate SQLite DBs, so `itr depend` can't span them."** This plan
is about what it would take to lift that caveat, and how far up the cost curve
it is worth going.

## Why it is hard (current itr model)

Every link primitive in `itr` is an **integer foreign key scoped to one file**,
and three behaviors depend on that scoping (verified against `itr` schema, 2026-07-05):

- **IDs are per-DB autoincrement.** `issues.id` is not globally unique — `#214`
  exists in multiple DBs. A cross-DB reference is meaningless without a
  `(db-identity, id)` locator.
- **`dependencies` / `relations` are enforced FKs** (`REFERENCES issues(id)`,
  `PRAGMA foreign_keys=ON`, `ON DELETE CASCADE`). SQLite foreign keys **cannot
  span database files**, even `ATTACH`ed ones — so a cross-DB edge cannot be
  integrity-checked by the engine.
- **Three behaviors assume a single in-file graph:**
  - `is_blocked` counts blockers whose status ≠ done/wontfix → needs the
    blocker's live status, now in another file.
  - **Cycle detection** walks `blocker→blocked` edges before every insert.
  - **Cascade-on-close** unblocks dependents / cascades edge deletes.
- **No atomicity.** One SQLite transaction cannot write two files with
  integrity → ACID is replaced by eventual reconciliation.
- **No addressing/discovery.** Resolving `wisphive#474` requires knowing *where*
  wisphive's DB is, and surviving moves/renames.

## Product Boundary

### In Scope

- Cross-DB **informational** links (`related` / `supersedes` / `duplicate`)
  between issues in different project trackers, authored and resolved from
  Wisphive.
- A fleet **registry** mapping a stable DB identity to a location, owned by the
  control plane (Wisphive already tracks every project's path).
- Navigation: from an issue in project A, see and open a linked issue in project
  B through the Wisphive UI.

### Out of Scope (hard constraint)

- **Cross-DB blocking / `depend`.** Dependency edges, `is_blocked`, cycle
  detection, and cascade-on-close stay **single-DB only**. Cross-project
  relationships are "this relates to that over there," not "this is
  transactionally gated on that." (Tier C below — deliberately not pursued.)
- A central issue server / global namespace (Tier D) — abandons `itr`'s
  local-file, zero-infra identity; explicit non-goal, consistent with the
  itr-repo DB-addressing spec's "no server mode."

## The cost curve (tiers)

Presented so the chosen boundary is not re-litigated. Most of these are
**upstream `itr` changes**; Wisphive owns the registry and the UI.

**Tier A — Soft external references** *(cheap, fits itr's soft-fallback ethos)*
- Upstream itr: a `db_id` UUID stamped into each DB (a `meta` row) so identity
  survives moves/renames; a locator grammar `alias#id` / `uuid#id`; a new
  `external_links` table `(local_id, target_db, target_id, relation_type, note)`
  with **no FK, no enforcement** — dangling tolerated and flagged by `doctor`,
  exactly like existing soft-fallbacks.
- `itr relate 474 --to wisphive#474 --type related` parses and stores a soft edge.
- Buys machine-readable, greppable cross-repo edges — ~90% of what "link across
  DBs" means in practice.

**Tier B — Registry + resolver** *(navigation & reverse links)*
- A fleet registry `alias → {uuid, path}`. **Owned by Wisphive** (or itr config).
- `itr get wisphive#474` (with a registry) opens the other DB read-only and
  renders it; reverse links computed by scanning registered DBs or a cached index.
- Links become bidirectional and navigable, still soft.

**Tier C — Cross-DB blocking semantics** *(expensive — explicitly declined)*
- `is_blocked` must consult external blockers' live status → open target DBs per
  query, and define "blocker DB unreachable" behavior (correct answer:
  *blocked-unknown, surfaced*, never silently ready). Cross-DB cycle detection
  needs every DB in the cycle reachable. Cascade/transactions replaced by an
  idempotent `itr fleet sync` reconciliation pass.
- Not worth the availability + complexity cost for the value delivered.

**Tier D — Central fleet DB / server** *(heaviest — non-goal)*
- A coordinating service owning a global namespace. Restores real integrity and
  atomic cross-project ops, but abandons the local-file identity. Out of scope.

## Recommendation

**Tier A + B, and stop there.** Soft `external_links` + a Wisphive-owned registry
gives navigable, honest cross-repo linking that matches `itr`'s philosophy
(local SQLite, agent-first, soft-fallback). Keep dependency/blocking single-DB.

The registry is the pivot: once `itr` can be addressed by `--db <dir>`
(itr#214 / [plan-fleet-itr-db-addressing.md](plan-fleet-itr-db-addressing.md)),
a thin `alias → path` map on top of Wisphive's existing project discovery (epic
itr#349) is what turns per-project addressing into fleet-wide linking. This plan
is therefore **sequenced after** DB addressing, and shares its ADR.

## Design (Tier A + B)

### Identity & locator

- Upstream itr stamps a `db_id` UUID into a `meta` table at `init` (and
  back-fills on first open of a legacy DB via a migration).
- Locator grammar: `<alias>#<id>` for humans (resolved through the registry) and
  `<uuid>#<id>` as the canonical stored form (survives alias/path changes).

### Registry (Wisphive-owned)

- Wisphive already scans/audits projects (epic itr#349). Extend that model to
  record, per project, `{alias, db_id (uuid), db_path}`.
- Expose the registry to `itr` invocations (env or `--registry <file>`), so
  `itr` can resolve/validate locators when Wisphive shells out.
- The registry is a **cache/index**, not a source of truth — a stale entry
  degrades to "unresolved locator," never to a wrong write.

### Edge storage & integrity posture

- Cross-DB edges live in the **source** DB's `external_links` (author's side),
  storing the target as `uuid#id`. No FK, no cascade.
- Reverse links are **derived**, not stored: Wisphive (or `itr --registry`) scans
  registered DBs' `external_links` to answer "what links *to* this issue."
- Dangling edges (target closed / DB gone) are tolerated and surfaced by `doctor`
  / a Wisphive health check — consistent with the soft-fallback philosophy.

### Wisphive UI

- When filing/relating from the control plane, offer "link to an issue in another
  project," resolving the target through the registry.
- On an issue detail surface, render its cross-DB links with the target's
  project alias and live title/status (fetched read-only), each navigable.
- Agent/issue text stays untrusted: render through the audited sanitizer, never
  `dangerouslySetInnerHTML` (CLAUDE.md web rule).

## Acceptance (workstream-level)

- `itr relate <local> --to <alias>#<id> --type related` (post-upstream) stores a
  soft edge; the local DB is unchanged except for the new `external_links` row;
  no FK error even when the target does not exist.
- From Wisphive, an issue in project A shows its link to project B's issue with
  B's live title/status, and the link opens B's issue read-only.
- A reverse-link query from B surfaces the A→B edge without B storing anything.
- Moving project B's DB to a new path (registry updated, `db_id` unchanged) keeps
  the A→B link resolvable.
- Deleting/closing the target degrades to a surfaced "dangling/closed" link,
  never a crash or a silent wrong resolution.
- **Negative:** no code path lets a cross-DB edge participate in `is_blocked`,
  cycle detection, or cascade (Tier C boundary is enforced, not just documented).

## Sequencing

1. Upstream DB addressing (itr#214) — prerequisite.
2. Upstream itr: `db_id` UUID + `external_links` (Tier A).
3. Wisphive registry on top of epic itr#349 discovery (Tier B).
4. Upstream itr: registry-aware resolver for `get`/`relate` (Tier B).
5. Wisphive UI: author + render + navigate cross-DB links.
6. `doctor` / Wisphive health surfacing of dangling edges.
