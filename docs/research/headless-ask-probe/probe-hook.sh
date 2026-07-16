#!/bin/sh
P="/tmp/wp559c.k5fu"
in=$(cat)
ev=$(printf '%s' "$in" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("hook_event_name",""))' 2>/dev/null)
tool=$(printf '%s' "$in" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("tool_name",""))' 2>/dev/null)
printf '%s %s\n' "$ev" "$tool" >> "$P/events.log"
if [ "$ev" = "PreToolUse" ] && [ "$tool" = "Bash" ] && [ "$MODE" != "control" ]; then
  printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask"}}'
  exit 0
fi
exit 0
