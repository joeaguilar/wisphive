#!/usr/bin/env bash
set -euo pipefail

project="${1:-.}"
wisphive_bin="${WISPHIVE_BIN:-wisphive}"

if [[ ! -d "$project" ]]; then
  echo "error: not a directory: $project" >&2
  exit 2
fi

project="$(cd "$project" && pwd -P)"

claude_events=(
  PreToolUse
  PostToolUse
  PermissionRequest
  Elicitation
  UserPromptSubmit
  Stop
  SubagentStop
  ConfigChange
  TeammateIdle
  TaskCompleted
)

codex_events=(
  PreToolUse
  PostToolUse
  PermissionRequest
  UserPromptSubmit
  Stop
)

seed_dir() {
  local rel="$1"
  local path="$project/$rel"
  if [[ -d "$path" ]]; then
    echo "skipped $rel (already present)"
  else
    mkdir -p "$path"
    echo "seeded  $rel"
  fi
}

seed_file() {
  local rel="$1"
  local body="$2"
  local path="$project/$rel"
  if [[ -e "$path" ]]; then
    echo "skipped $rel (already present)"
  else
    printf '%s\n' "$body" > "$path"
    echo "seeded  $rel"
  fi
}

has_event_hooks() {
  local file="$1"
  shift

  [[ -f "$file" ]] || return 1
  grep -q 'wisphive' "$file" || return 1

  local event
  for event in "$@"; do
    grep -q "\"$event\"" "$file" || return 1
  done
}

seed_hooks() {
  local claude_settings="$project/.claude/settings.json"
  local codex_hooks="$project/.codex/hooks.json"

  if has_event_hooks "$claude_settings" "${claude_events[@]}" &&
    has_event_hooks "$codex_hooks" "${codex_events[@]}"; then
    echo "skipped Wisphive hooks (already present)"
    return
  fi

  "$wisphive_bin" hooks install --project "$project"
  echo "seeded  Wisphive hooks"
}

seed_dir ".claude"
seed_dir ".codex"
seed_hooks
seed_file "CLAUDE.md" "# Project Instructions

Shared agent guidance for this project. Keep this file aligned with AGENTS.md when changing project workflow, CLI defaults, runtime files, hooks, or safety-critical behavior."
seed_file "AGENTS.md" "# AGENTS.md

Shared instructions for Codex and other AI agents working in this project. Keep this file aligned with CLAUDE.md when changing project workflow, CLI defaults, runtime files, hooks, or safety-critical behavior."
