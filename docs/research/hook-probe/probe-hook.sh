#!/bin/sh
# Pure logging hook for the ExitPlanMode/AskUserQuestion emission probe (itr#459).
# Captures which hook events Claude Code actually fires — emits NO decision, so it
# never alters behavior (PreToolUse falls through to normal permissions;
# PermissionRequest lets the native dialog show). Appends one summary line per
# event plus the full raw stdin to a raw log for forensics.
LOG=/private/tmp/claude-501/-Users-josefaguilar-AI-Projects-wisphive/20cf1f47-3db4-4a76-9a13-d569b9941c9c/scratchpad/hook-probe/probe.log
RAW=/private/tmp/claude-501/-Users-josefaguilar-AI-Projects-wisphive/20cf1f47-3db4-4a76-9a13-d569b9941c9c/scratchpad/hook-probe/probe-raw.jsonl
input=$(cat)
ts=$(date '+%H:%M:%S')
line=$(printf '%s' "$input" | python3 -c 'import sys,json
try:
    d=json.load(sys.stdin)
    print("event=%-18s tool=%s" % (d.get("hook_event_name",""), d.get("tool_name","")))
except Exception as e:
    print("PARSE_ERROR", e)' 2>/dev/null)
printf '%s  %s\n' "$ts" "$line" >> "$LOG"
printf '%s\n' "$input" >> "$RAW"
exit 0
