# Investigation: Empty Detail Views for Non-Standard Tool Events

## Problem

Several Claude Code tool events arrive in the TUI detail view with no visible content — just the header (Agent, Project, Tool, Time) and an empty body. The user cannot make an informed decision.

### Affected Tools

#### ExitPlanMode
- **Observed**: Detail view is empty. No plan content visible.
- **Status bar**: Shows PreToolUse keybindings (Y/N/M/!/E/C/?) — not plan-specific.
- **Root cause**: `tool_input` for ExitPlanMode is `{}` or `null`. The plan content is Claude's assistant text output during plan mode, not a tool parameter.
- **Claude Code docs say**: "This tool does NOT take the plan content as a parameter — it will read the plan from the file you wrote."
- **Hook visibility**: Issue [#21282](https://github.com/anthropics/claude-code/issues/21282) reports that PreToolUse/PostToolUse hooks don't fire for plan mode tools at all. However, we ARE seeing these events — possibly coming through as PermissionRequest events or behavior has changed.
- **Historical note**: In v2.0.34, `tool_input` contained `{"plan": "..."}` but this was removed/changed. See [#12288](https://github.com/anthropics/claude-code/issues/12288).

#### AskUserQuestion
- **Observed**: Detail view is empty. Shows `[1-0]select` status bar (PermissionRequest keybindings) but no question content.
- **Root cause (CONFIRMED via debug logging 2026-03-22)**:
  - AskUserQuestion fires 3 events: PreToolUse, PermissionRequest, PostToolUse.
  - The PermissionRequest event has **no `permission_suggestions`** field.
  - The question data IS in `tool_input.questions[]` with fields: `question`, `header`, `options[].label`, `options[].description`, `multiSelect`.
  - The TUI's `push_permission_detail` only renders `permission_suggestions`, so the view is empty.
  - The `[1-0]select` status bar is wrong — there are no suggestions to select from.
- **Fix**: Detect PermissionRequest with no suggestions but with `tool_input.questions` and render the question content with appropriate keybindings (approve/deny instead of 1-0 select).

### Potential Other Affected Tools
- `EnterPlanMode`, `EnterWorktree`, `ExitWorktree` — any tool with minimal/empty `tool_input` that falls through to `push_generic_detail`.

## Data Availability

### What's in the hook stdin
Standard fields: `session_id`, `tool_name`, `tool_use_id`, `tool_input`, `cwd`, `permission_mode`, `hook_event_name`, `transcript_path`.

### Key field: `transcript_path`
Currently **ignored** by wisphive-hook. This JSONL file contains the full conversation including assistant text. Could be used to extract plan content for ExitPlanMode.

### `tool_input` contents (VERIFIED via debug logging 2026-03-22)
- **ExitPlanMode**: Not yet captured. Expected `{}` based on system prompt docs.
- **AskUserQuestion**: `{"questions": [{"question": "...", "header": "...", "options": [{"label": "...", "description": "..."}], "multiSelect": bool}]}`. Confirmed via PreToolUse and PermissionRequest payloads.
- **EnterPlanMode**: Not yet captured.

## Plan

1. Add debug logging to dump raw hook stdin to `~/.wisphive/hook-debug.jsonl`
2. Trigger ExitPlanMode and AskUserQuestion events to capture actual payloads
3. Based on findings:
   - Add dedicated detail renderers for each tool type
   - Extract `transcript_path` if needed for plan content
   - Ensure action hints match the event semantics

## Sources
- [ExitPlanMode system prompt](https://github.com/Piebald-AI/claude-code-system-prompts/blob/main/system-prompts/tool-description-exitplanmode.md)
- [Bug: ExitPlanMode empty tool_input](https://github.com/anthropics/claude-code/issues/12288)
- [Feature request: Plan mode hooks](https://github.com/anthropics/claude-code/issues/21282)
- [Bug: PermissionRequest + ExitPlanMode](https://github.com/anthropics/claude-code/issues/15755)
