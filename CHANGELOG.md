# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - Unreleased

### Added
- Daemon server with Unix socket IPC and SQLite persistence
- Hook binary for Claude Code PreToolUse/PostToolUse/PermissionRequest events
- TUI dashboard with queue, agents, and projects panels
- Web UI with real-time WebSocket updates
- Auto-approve system with tiered levels (off/read/write/execute/all)
- Content-aware tool rules (deny/allow patterns per tool)
- Decision history with full-text search
- Session and project aggregation views
- Desktop notifications (macOS/Linux)
- Agent process spawning and management
- Event logging with retention and archival
- Fail-open design (hook errors always approve)
