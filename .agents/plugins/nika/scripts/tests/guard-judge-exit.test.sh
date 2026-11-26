#!/usr/bin/env bash
# A broken judge must not turn permission-looking stdout into an allow.
# Exercise the installed shim, not a copy of its dispatch, in both dialects.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHIM="$HERE/../guard-run.sh"
judge_root="$(mktemp -d)"
cleanup() {
  rm -f "$judge_root/nika"
  rmdir "$judge_root"
}
trap cleanup EXIT
# The stub writes its answer and the end of line nika guard writes after
# one, unless GUARD_TEST_CUT is set: then the line stops short, the way a
# judge that dies while writing leaves it.
cat >"$judge_root/nika" <<'EOF'
#!/bin/sh
/bin/cat >/dev/null
if [ -n "$GUARD_TEST_OUTPUT" ]; then
  if [ -n "${GUARD_TEST_CUT:-}" ]; then
    printf '%s' "$GUARD_TEST_OUTPUT"
  else
    printf '%s\n' "$GUARD_TEST_OUTPUT"
  fi
fi
exit "$GUARD_TEST_EXIT"
EOF
chmod +x "$judge_root/nika"
fails=0
checks=0

# judge <dialect> <exit> <payload> <answer> <expected> [cut]: the shim's
# outcome when the stub answers <answer>, its line cut short when the
# sixth argument is given.
judge() {
  local dialect="$1" code="$2" payload="$3" supplied="$4" expected="$5" cut="${6:-}" out rc=0
  checks=$((checks + 1))
  out="$(printf '%s' "$payload" | env -i PATH="$judge_root:/usr/bin:/bin" \
    HOME="$judge_root/home" XDG_STATE_HOME="$judge_root/state" GUARD_TEST_CUT="$cut" \
    GUARD_TEST_OUTPUT="$supplied" GUARD_TEST_EXIT="$code" /bin/bash "$SHIM" 2>&1)" || rc=$?
  if [ "$expected" = unavailable ]; then
    # Each refusal names its real cause. The binary is on PATH in every
    # case here, so none of them may send the person to restore it.
    cause='did not give a verdict'
    [ "$code" -ne 3 ] || [ -z "$supplied" ] || cause='could not judge this command'
    if [ "$rc" -eq 2 ] && [[ "$out" == *guard_unavailable* ]] \
      && [[ "$out" == *"$cause"* ]] && [[ "$out" != *"not on PATH"* ]]; then
      printf 'ok    %s exit %s refuses (%s)\n' "$dialect" "$code" "$cause"
      return
    fi
  elif [ "$rc" -eq 0 ] && [ "$out" = "$expected" ]; then
    printf 'ok    %s exit %s preserves verdict\n' "$dialect" "$code"
    return
  fi
  printf 'FAIL  %s exit %s: expected %s, got %s\n' \
    "$dialect" "$code" "$expected" "$out" >&2
  fails=$((fails + 1))
}

for dialect in cursor claude; do
  if [ "$dialect" = cursor ]; then
    payload='{"command":"nika run fixture.nika","cwd":"/tmp"}'
    unrelated='{"command":"ls","cwd":"/tmp"}'
    allow='{"permission":"allow"}'
    deny='{"permission":"deny","agent_message":"fixture judge refused"}'
  else
    payload='{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"nika run fixture.nika"},"cwd":"/tmp"}'
    unrelated='{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"/tmp"}'
    allow='{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}'
    deny='{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"fixture judge refused"}}'
  fi
  judge "$dialect" 0 "$payload" "$allow" "$allow"
  judge "$dialect" 2 "$payload" "$deny" "$deny"
  judge "$dialect" 3 "$payload" "$deny" unavailable
  for code in 1 4 127 255; do
    judge "$dialect" "$code" "$payload" "$allow" unavailable
  done
  for code in 0 2 3; do
    judge "$dialect" "$code" "$payload" '' unavailable
  done
  judge "$dialect" 0 "$unrelated" '{}' '{}'
  judge "$dialect" 1 "$unrelated" "$allow" unavailable
  # A verdict is one whole line. Cut short, with a second line, or as a
  # piece of an object (here a deny whose outer brace never came), the
  # answer is no verdict, whatever the exit; a deny on exit 0 still blocks.
  for code in 0 2; do
    judge "$dialect" "$code" "$payload" "$allow" unavailable cut
    judge "$dialect" "$code" "$payload" "$deny" unavailable cut
    judge "$dialect" "$code" "$payload" "${deny%\}}" unavailable
    judge "$dialect" "$code" "$payload" "$deny$deny" unavailable
    judge "$dialect" "$code" "$payload" "$allow
$deny" unavailable
  done
  judge "$dialect" 0 "$payload" '{}' unavailable cut
  judge "$dialect" 0 "$payload" "$deny" "$deny"
  # On exit 2 the judge denied: an answer that does not deny is refused.
  judge "$dialect" 2 "$payload" '{}' unavailable
  judge "$dialect" 2 "$payload" "$allow" unavailable
  judge "$dialect" 2 "$payload" '{"reason":"\"permission\":\"deny\""}' unavailable
  # Encode the payload through a real JSON writer; never execute the command.
  for command in "n'ik'a run x.yaml" 'n\ika run x.yaml' 'echo hook_event_name; ls'; do
    opaque="$(jq -nc --arg dialect "$dialect" --arg command "$command" '
      if $dialect == "cursor" then {command:$command,cwd:"/tmp"}
      else {hook_event_name:"PreToolUse",tool_name:"Bash",tool_input:{command:$command},cwd:"/tmp"} end')"
    judge "$dialect" 1 "$opaque" "$allow" unavailable
  done
done

if [ "$fails" -ne 0 ]; then
  printf 'FAIL  %s of %s judge-exit controls\n' "$fails" "$checks" >&2
  exit 1
fi
printf 'OK  %s judge-exit controls; no workflow or effect was executed\n' "$checks"
