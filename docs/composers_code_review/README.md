# Code review notes

Independent pass over the Wisphive workspace (Rust crates + React frontend) for bugs, reliability issues, security-adjacent behavior, and maintainability anti-patterns.

**Scope:** Static review (read source, grep patterns). No full threat model or penetration test.

**Organization:**

| Document | Focus |
|----------|--------|
| [hook-and-ipc.md](./hook-and-ipc.md) | `wisphive-hook`, wire protocol, fail-open semantics |
| [daemon-and-state.md](./daemon-and-state.md) | Daemon server, SQLite queue vs pending rows, notifications |
| [web-and-frontend.md](./web-and-frontend.md) | `wisphive_web`, auth/security, SPA (`MarkdownText`, etc.) |
| [cli-and-adapters.md](./cli-and-adapters.md) | CLI footguns, hooks installer, adapters / spawned agents |

*(You originally asked for `docs/code_reivew`; this folder uses the conventional spelling `docs/code_review`.)*

**Severity legend:** *Critical* = incorrect behavior or meaningful security risk in plausible use; *High* = significant reliability or policy surprise; *Medium* = worth fixing or documenting; *Low* = polish / edge cases.
