# campaign-002 sprint5-residue — orchestrator notes

- Approved 2026-07-14 by PO with two amendments confirmed:
  - #529/#530 are decide-or-fix: implement if cheap and portable (injective `\\` self-escape for #530; test seam for #529), else record accepted-gap rationale on the issue.
  - #528 deliverable is ADR + sizing only, no code.
- Branch: crossfire-blitz/20260712-230112 (active sprint-5 workstream). Conventional Commits per bundle, stage only owned files.
- Wave plan: W1 = Q-1 Q-2 Q-4 Q-5 Q-6 · W2 = Q-3 (ADR) + Q-7 (process_registry refactor, deferred to avoid overlapping Q-3's read).
- Full `just verify` runs from orchestrator between waves; workers run targeted gates only.
- Known ambient flake: itr#520/#525-family machine-load test flakiness — W1's Q-1 is itself the fix for part of it.
