# Documentation Strategy — Wisphive

_Last reviewed: 2026-06-15_

> **Start here.** This file is the map of every documentation surface in Wisphive: what each one
> is for, who keeps it current, and **how they link together**. If you are an agent picking up a
> task and you need context the task doesn't carry, this is the index to traverse.

## The problem this solves

Wisphive is heavily documented — `CLAUDE.md`/`AGENTS.md`, `docs/ROADMAP.md`, the `itr` backlog,
`docs/decisions/`, `docs/handoff/`, `docs/research/`, the `docs/plan-*.md` workstream docs, and
machine-local `~/.claude` memory. The gap was never *missing* documentation; it was
**discoverability**: an agent holding an `itr` task had no reliable way to find the decision behind
it, the handoff that shipped the surrounding work, or the roadmap section that frames it. The
surfaces existed but weren't wired together.

The fix is two things:

1. **One obvious entry point** — this file, reachable in two hops from the files every agent
   already reads (`CLAUDE.md` / `AGENTS.md` → "Documentation Map" → here).
2. **Consistent cross-links** — every surface links to the others (see
   [Cross-linking rules](#cross-linking-rules-the-spine)), so any one can be traversed to the rest.

## Documentation map — "I'm looking for…"

| I need… | Go to |
|---------|-------|
| **How to build / test / run + how a subsystem works** | `CLAUDE.md` (canonical) / `AGENTS.md` (Codex mirror) |
| **A task to work on** | `itr` — `itr ready`, `itr next`, `itr get <ID>` |
| **What's done / in flight / next** | `docs/ROADMAP.md` (section grain, ✅/🟡/❌, links itr IDs) + `itr ready` (task grain) |
| **Why a design is the way it is** | `docs/decisions/` (ADRs) — start at [`docs/decisions/README.md`](decisions/README.md) |
| **What v1 is / scope boundary** | `CLAUDE.md` (de-facto spec) + `docs/ROADMAP.md` "Release boundary" |
| **What happened in a past milestone** | `docs/handoff/YYYY-MM-DD-<topic>.md` |
| **An upcoming workstream's design** | `docs/plan-*.md` (cross-agent conflict gate, deterministic analytics, decision plugins, policy learning, mobile pairing, red support) |
| **Exploratory analysis / landscape scans** | `docs/research/` |
| **OSS positioning and roadmap** | `docs/open-source-path.md` |
| **TUI widget / investigation reference notes** | `claude/` (tui-textarea reference, empty-detail-views investigation) |
| **A superseded / completed-and-stale doc** | `docs/archive/` (see `archive/README.md`) |
| **Agent working-context notes** | `~/.claude/.../memory/` (machine-local, **not** in the repo) |

## Source-of-truth model

Every documentation concern has **exactly one** home. Don't duplicate a concern across surfaces;
link instead.

| Concern | Single source of truth | Kept fresh by | When |
|---------|------------------------|---------------|------|
| Architecture / build / agent guidance | `CLAUDE.md` (canonical); `AGENTS.md` mirrors for Codex | author of the change | with the change |
| Scope / "what v1 is" | `CLAUDE.md` (de-facto spec) + ROADMAP release boundary | PO | on scope change |
| Roadmap (done/in-flight/next, section grain) | `docs/ROADMAP.md` | `/roadmap`, `scripts/roadmap_sync_check.py`, `/sprint-review` | every sprint |
| Backlog (task grain) | `itr` | everyone | continuous |
| **Decisions (the _why_)** | `docs/decisions/ADR-NNNN-*.md` + index | the decider | at decision time |
| Milestone narratives | `docs/handoff/YYYY-MM-DD-*` | implementer | per milestone |
| Upcoming-workstream designs | `docs/plan-*.md` | author | as the design evolves |
| Exploratory research / landscape | `docs/research/*` → `docs/archive/` when superseded | researcher | ad hoc |
| Reference notes (TUI widgets, investigations) | `claude/*` | author | ad hoc |
| Agent working context | `~/.claude` memory (machine-local) | agent | continuous |

**Why decisions get a repo home (not just memory):** `~/.claude` memory is machine-local — invisible
to a fresh clone, to collaborators, and to a reviewing agent on another machine. Decisions that
constrain future work must be durable and shared, so they live in git as ADRs. Memory then points
*to* the ADR (agent working-context, not the canonical archive).

## Decision records (ADRs)

- **Location:** `docs/decisions/`
- **Naming:** `NNNN-short-kebab-title.md` (4-digit, zero-padded, monotonic)
- **Template:** [`docs/decisions/0000-template.md`](decisions/0000-template.md)
- **Index:** [`docs/decisions/README.md`](decisions/README.md) — every ADR has a row; newest filed last

**File an ADR when a decision** (a) constrains future work, (b) was non-obvious / had real
alternatives, or (c) someone will later ask "why is it done this way." Minimum viable ADR is half
a page — context, decision, rationale, consequences, alternatives, links.

**Status lifecycle:** `Proposed` → `Accepted` → `Superseded by ADR-XXXX` / `Deprecated`. Never delete
a superseded ADR; flip its status and link the successor. The reasoning history is the point.

## Dated docs: handoff vs research

Both are dated, append-only, never rewritten in place.

- **`docs/handoff/`** — milestone breadcrumbs: what shipped, the trade-offs made, the hard rules
  established, what's left for the next implementer. Write one when closing an epic/phase or handing
  off mid-stream. Copy `docs/handoff/TEMPLATE.md` to start; get the facts from git, not from memory.
- **`docs/research/`** — *pre-decision* exploration, feasibility analysis, and landscape scans. When
  a research doc's conclusion is acted on, the resulting decision should become an ADR that links
  back to it.

## Plan docs

`docs/plan-*.md` hold the design for upcoming or deferred workstreams (cross-agent conflict gate,
deterministic agent analytics, decision plugins, policy learning engine, mobile device pairing, Red
support). They are living design docs — when a plan's decision is locked, distill it into an ADR and
let the plan doc carry the elaboration. Each carries a `_Last reviewed: YYYY-MM-DD_` line near the
top.

## Archival

Superseded or completed-and-stale docs move to `docs/archive/` (keep the original filename) rather
than rotting among live docs. A doc is archive-eligible when its subject shipped and the doc is no
longer a reference. Living docs carry a `_Last reviewed: YYYY-MM-DD_` line near the top so staleness
is machine-checkable.

## Cross-linking rules (the spine)

This is what makes the surfaces traversable — the actual fix for the discoverability gap:

- **ADRs** link: originating `itr` IDs, the commit(s) / handoff that implemented them, the code
  paths they govern, and related ADRs.
- **ROADMAP** section notes reference `ADR-NNNN` where a section embodies a decision (in addition to
  the `itr` IDs already linked).
- **Handoff docs** add a one-line **Decisions:** entry linking any ADR filed or affected.
- **itr issues** reference `ADR-NNNN` in their `context` when the task implements or depends on a
  decision.
- **Memory `project_*`** files point to their canonical ADR.

Net effect: from any task you are at most one hop from its decision, its milestone, and its roadmap
section.

## Keeping it fresh

- **Ad-hoc work (fixes, spikes, one-off tasks outside a `/sprint` or `/blitz` — the common case):**
  at the **end of the task, before you call it done**, decide whether the change earns a doc update
  and make it *then* — no ceremony is coming to catch it. Quick checklist:
  - Made a non-obvious design decision? → file/refresh an **ADR** and link it from the `itr` issue.
  - Changed how a subsystem works, builds, or runs? → update **`CLAUDE.md`** (and mirror to `AGENTS.md`).
  - Shipped or closed out a section/feature? → reflect it in **`ROADMAP.md`** (or run `/roadmap`).
  - Finished a milestone or handed off mid-stream? → drop a **`docs/handoff/`** note.
  - "No doc change needed" is a valid answer — just make it a conscious decision, not an omission.
- **`/sprint` (Phase 0):** read `docs/ROADMAP.md` and this map.
- **`/sprint-review`:** updates ROADMAP and files/refreshes ADRs for decisions made this sprint.
- **`just docs-lint` / `scripts/roadmap_sync_check.py`:** reconcile ROADMAP ↔ `itr` ↔ crates and
  flag structural drift in the doc surfaces.

> `docs-lint` catches *structural* drift. It cannot catch *prose* drift — a doc whose words no
> longer match reality. That is exactly what the end-of-task check above is for.

## File tree

```
docs/
├── DOCUMENTATION.md             ← you are here (the index)
├── ROADMAP.md                   ← done / in-flight / next (section grain)
├── open-source-path.md          ← OSS positioning and roadmap
├── plan-cross-agent-conflict-gate.md   ← upcoming-workstream designs
├── plan-deterministic-agent-analytics.md
├── plan-decision-plugins.md
├── plan-mobile-device-pairing.md
├── plan-policy-learning-engine.md
├── plan-red-support.md
├── decisions/                   ← ADRs (the "why")
│   ├── README.md                ← ADR index
│   ├── 0000-template.md
│   └── NNNN-*.md
├── handoff/                     ← milestone narratives (dated)
│   ├── TEMPLATE.md
│   └── YYYY-MM-DD-*.md
├── archive/                     ← superseded docs (see archive/README.md)
├── composers_code_review/
└── research/                    ← pre-decision exploration / landscape scans

claude/                          ← reference notes (top-level, outside docs/)
├── tui-textarea-reference.md
└── investigation-empty-detail-views.md
```

## Open / deferred

- Formalize a `docs/SPEC.md` scope contract (today `CLAUDE.md` is the de-facto spec).
- Backfill ADRs for the remaining candidate decisions listed in
  [`docs/decisions/README.md`](decisions/README.md) as those areas are next touched.
