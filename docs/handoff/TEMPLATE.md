<!--
HANDOFF TEMPLATE — copy this file to docs/handoff/YYYY-MM-DD-<topic>.md and fill it in.

Rules of the road:
- Append-only. Once written, a handoff is a milestone breadcrumb — never rewrite
  it in place. If the situation changes, write a NEW dated handoff and link back
  to this one as the predecessor.
- Write one when you close an epic/phase, OR when you hand off mid-stream and the
  next implementer (a fresh clone, a collaborator, a reviewing agent on another
  machine) would otherwise have to re-derive months of subtle invariants.
- Be a genuine breadcrumb, not filler. State what shipped, the trade-offs you
  made, and exactly where the next person should start.
- Get the facts from git, not from memory: `git show --stat <sha>`, `git log`.
- Delete these HTML comments before committing your filled-in copy.
-->

# <Topic> — handoff & next steps

**Date:** YYYY-MM-DD
**Branch:** `main` @ `<short-sha>`
**Epic / itr:** itr#<N> (and any sub-issues)
**Closed this session:** itr#<…>
**Filed this session:** itr#<…>
**Predecessor handoff:** `docs/handoff/<prev>.md` (or "none — first handoff")

If you only have 60 seconds: <one or two sentences pointing the reader at the
single most important thing — usually "scroll to § Where to start next" plus the
one decision or risk that gates everything else.>

## What just shipped

<A plain-language paragraph: what can a user/operator now do that they could not
before? Then the commit(s) and an anchor table so the next reader can jump
straight to the load-bearing code.>

```
<short-sha>  <commit subject line>
```

| Surface | Anchor |
|---|---|
| <what> | `crates/.../file.rs` |

## Trade-offs made

<Decisions where you picked one path over another, and why. The "why" matters as
much as the "what" — future sessions will hit edge cases the decision doesn't
literally cover. Call out anything destructive-if-missed explicitly.>

## What's NOT shipped — explicit scope gaps

<Known gaps, filed as itr follow-ups where applicable. Not blocking closure, but
the next implementer should know they exist before they get surprised.>

1. **<gap>** (itr#<N>, <severity>). <one-line description.>

## Hard rules established this session

<Numbered invariants this change locked in. "DO NOT regress." These catch
well-meaning refactors that would quietly break a security or correctness
property.>

## Where to start next

<The single most useful thing in the file. Give 1–3 concrete entry points,
sized, with the recommended one called out and a first-wedge checklist if useful.>

## Memory / docs to read for context

<Pointers to ~/.claude memory entries, CLAUDE.md sections, or other docs the next
session should read before touching this area.>
