# Contributing to Wisphive

Thank you for your interest in contributing to Wisphive!

## Prerequisites

- **Rust nightly** — Wisphive uses edition 2024, which requires a recent nightly toolchain
- **Node.js 18+** — for the web frontend (React/TypeScript/Vite)
- **macOS or Linux** — the daemon uses Unix domain sockets

## Building

```bash
cargo build --workspace          # Build all crates (debug)
cargo build --release            # Build release binaries
./install.sh                     # Build release + install to ~/.cargo/bin + codesign on macOS
```

Two binaries are produced: `wisphive` (CLI/daemon/TUI) and `wisphive-hook` (Claude Code hook subprocess).

## Testing

```bash
cargo test --workspace           # Run all tests
cargo test -p wisphive_daemon    # Test a single crate
cargo test server_cleans_up      # Run a single test by name
```

## Linting

```bash
cargo clippy --workspace -- -D warnings   # Must pass with zero warnings
```

## Frontend Development

```bash
cd crates/wisphive_web/frontend
npm install
npm run dev       # Vite dev server on :5173 (proxies WebSocket to daemon)
npm run build     # Production build (embedded into Rust binary via rust-embed)
npm run lint      # ESLint
```

## Pull Request Guidelines

- One logical change per PR
- Include tests for new functionality
- Ensure `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` pass
- Write descriptive PR titles (e.g., "fix: resolve duplicate history entries on refresh")

## Issue Tracking

This project uses `itr` for local issue tracking. See the `.itr.db` file (gitignored) for current issues.

## Architecture

See [CLAUDE.md](CLAUDE.md) for a detailed architecture overview, crate dependency flow, and key design decisions.
