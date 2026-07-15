_default:
    @just --list

# Build all crates (debug)
build:
    cargo build --workspace

# Build release binaries
release:
    cargo build --release

# Run the full install script (release build + copy to ~/.cargo/bin + codesign)
install:
    ./install.sh

# Run all tests
test:
    cargo test --workspace

# Run a single test by name
test-one name:
    cargo test {{name}}

# Lint
clippy:
    cargo clippy --workspace

# Format
fmt:
    cargo fmt --all

# Lint the docs/ROADMAP.md <-> itr <-> crates seam (deterministic drift check)
docs-lint:
    python3 scripts/roadmap_sync_check.py

# Start the daemon in the foreground
daemon:
    wisphive daemon start

# Stop the daemon
daemon-stop:
    wisphive daemon stop

# Daemon status
status:
    wisphive daemon status

# Open the TUI
tui:
    wisphive tui

# Start the web UI server (production — serves embedded frontend assets)
web port="8080" host="127.0.0.1":
    wisphive web serve --host {{host}} --port {{port}}

# Start the web UI in dev mode (WebSocket only — run `just frontend-dev` in another terminal)
web-dev port="8080":
    wisphive web serve --dev --port {{port}}

# Start the daemon with the web UI in the same process
daemon-web port="8080" host="127.0.0.1":
    wisphive daemon start --web --host {{host}} --port {{port}}

# Install frontend dependencies
frontend-install:
    cd crates/wisphive_web/frontend && npm install

# Run the Vite dev server for the frontend (pair with `just web-dev`)
frontend-dev:
    cd crates/wisphive_web/frontend && npm run dev

# Build the frontend for embedding into the release binary
frontend-build:
    cd crates/wisphive_web/frontend && npm run build

# Lint the frontend
frontend-lint:
    cd crates/wisphive_web/frontend && npm run lint

# Run the frontend Vitest suite (jsdom + React Testing Library)
frontend-test:
    cd crates/wisphive_web/frontend && npm test

# Playwright e2e smoke suite (headless Chromium). Builds the frontend +
# a debug `wisphive` binary, then boots `wisphive web serve` against a
# fresh isolated HOME on an ephemeral port — never touches ~/.wisphive.
e2e *args="":
    cd crates/wisphive_web/frontend && npm run build
    cargo build -p wisphive_cli --bin wisphive
    cd crates/wisphive_web/frontend && npx playwright install chromium
    cd crates/wisphive_web/frontend && npx playwright test {{args}}

# Full verification gate suite. Every gate runs under its own gatr tag so
# `gatr last` / `gatr errors` prove the gate was green after the fact.
# Fail-fast: the first red gate aborts the recipe. Ordered so the debug
# cargo artifacts from verify-rust are reused by the e2e build step.
# TUI snapshot tests (crates/wisphive_tui/tests/) run inside verify-rust.
verify:
    gatr run --tag verify-fmt -- cargo fmt --all --check
    gatr run --tag verify-clippy -- cargo clippy --workspace -- -D warnings
    gatr run --tag verify-rust -- cargo test --workspace
    gatr run --tag verify-frontend -- bash -c 'cd crates/wisphive_web/frontend && npm run lint && npm test'
    gatr run --tag verify-e2e -- just e2e

# Install Claude Code hooks into the current project
hooks-install:
    wisphive hooks install --project .

# Uninstall hooks from the current project
hooks-uninstall:
    wisphive hooks uninstall --project .

# Enable hooks globally
hooks-enable:
    wisphive hooks enable

# Disable hooks globally
hooks-disable:
    wisphive hooks disable

# Hooks status
hooks-status:
    wisphive hooks status

# Full onboarding: install binaries, install hooks in cwd, enable
bootstrap: install hooks-install hooks-enable
    @echo "Wisphive ready. Run 'just daemon' in one terminal and 'just tui' in another."

# One-shot: build frontend, rebuild+install binaries, restart daemon with web UI.
# Browse to https://localhost:3100 (self-signed — accept the cert warning).
all host="127.0.0.1" port="3100":
    cd crates/wisphive_web/frontend && npm install && npm run build
    ./install.sh
    -wisphive daemon stop
    wisphive hooks install --project .
    @echo "Starting: https://{{host}}:{{port}}"
    wisphive daemon start --host {{host}} --port {{port}}

# Rebuild + reinstall + restart daemon (dev iteration)
reinstall:
    ./install.sh
    -wisphive daemon stop
    @echo "Run 'just daemon' to start fresh."

# Emergency kill switch
off:
    wisphive emergency-off

# Binary-independent rescue: diagnose the strict ~/.wisphive state the hook
# enforces (works even when every wisphive binary is broken). Also: --fix, --off.
rescue *ARGS:
    sh scripts/wisphive-rescue.sh {{ARGS}}

# Doctor / health check
doctor:
    wisphive doctor

# Red-team suites against release binaries in isolated throwaway HOMEs:
# decision-plane integrity (epic #403: ghost-approval, crash mid-stream,
# secret redaction) + upgrade safety (epic #533/#539: legacy perms deny
# deliberately with repair guidance, brick detector, doctor/rescue repair,
# install.sh preflight, UserPromptSubmit stays fail-closed).
redteam:
    cargo build --release --bin wisphive --bin wisphive-hook
    ./scripts/redteam-decision-plane.sh
    ./scripts/redteam-upgrade-safety.sh
