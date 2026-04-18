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
    wisphive web --host {{host}} --port {{port}}

# Start the web UI in dev mode (WebSocket only — run `just frontend-dev` in another terminal)
web-dev port="8080":
    wisphive web --dev --port {{port}}

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

# Rebuild + reinstall + restart daemon (dev iteration)
reinstall:
    ./install.sh
    -wisphive daemon stop
    @echo "Run 'just daemon' to start fresh."

# Emergency kill switch
off:
    wisphive emergency-off

# Doctor / health check
doctor:
    wisphive doctor
