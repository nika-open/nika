#!/usr/bin/env bash
# author-stop-gate.test · the SubagentStop gate of nika-author, played
# against fixture payloads and a stub binary (no model, no real run).
#
# Laws under test:
#   · another subagent is never judged;
#   · a clean, named workflow lets the author stop;
#   · findings send it back (exit 2, the code on stderr), twice at most,
#     then it is let through with a warning the person sees;
#   · a workflow written and not named in the return sends it back;
#   · a missing binary, a broken payload and a transcript that is not
#     there are warnings or silence, never a block.
#
# Usage: author-stop-gate.test.sh  (exit 0 = all green)
set -uo pipefail

command -v python3 >/dev/null 2>&1 || {
  echo "FAIL author-stop-gate.test: python3 is absent, so nothing was asserted" >&2
  exit 1
}

HERE="$(cd "$(dirname "$0")/.." && pwd)"
GATE="$HERE/author-stop-gate.py"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
FAILS=0
ok() { echo "ok   $1"; }
bad() {
  echo "FAIL $1"
  FAILS=$((FAILS + 1))
}
expect() { # expect <label> <want-rc> <got-rc> <haystack> <needle or empty>
  if [ "$2" != "$3" ]; then
    bad "$1 — exit $3, wanted $2"
  elif [ -n "$5" ] && ! printf '%s' "$4" | grep -qF -- "$5"; then
    bad "$1 — missing: $5"
  else ok "$1"; fi
}

# A stub judge: a file holding the word "broken" is refused with one finding.
mkdir -p "$ROOT/bin" "$ROOT/proj/workflows" "$ROOT/data"
cat >"$ROOT/bin/nika" <<'STUB'
#!/usr/bin/env bash
[ "$1" = check ] || exit 64
if grep -q broken "$2" 2>/dev/null; then
  printf '%s\n' '{"clean":false,"findings":[{"code":"NIKA-AUTH-006","message":"no grant for this write"}]}'
  exit 2
fi
printf '%s\n' '{"clean":true,"findings":[]}'
STUB
chmod +x "$ROOT/bin/nika"
printf 'nika: good\ntasks: {}\n' >"$ROOT/proj/workflows/good.nika"
printf 'nika: bad\n# broken\ntasks: {}\n' >"$ROOT/proj/workflows/bad.nika"

transcript() { # transcript <name> <written path or empty>
  : >"$ROOT/$1.jsonl"
  [ -n "$2" ] && printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Write","input":{"file_path":"%s"}}]}}\n' "$2" >>"$ROOT/$1.jsonl"
  printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}\n' >>"$ROOT/$1.jsonl"
}
payload() { # payload <agent_type> <agent_id> <transcript name> <message>
  printf '{"hook_event_name":"SubagentStop","cwd":"%s","agent_type":"%s","agent_id":"%s","agent_transcript_path":"%s","last_assistant_message":"%s"}' \
    "$ROOT/proj" "$1" "$2" "$ROOT/$3.jsonl" "$4"
}
gate() { NIKA_BIN="$ROOT/bin/nika" CLAUDE_PLUGIN_DATA="$ROOT/data" python3 "$GATE" 2>"$ROOT/err"; }

transcript t-good "$ROOT/proj/workflows/good.nika"
transcript t-bad "$ROOT/proj/workflows/bad.nika"
transcript t-none ""

out="$(payload "nika:nika-debugger" a0 t-bad "see workflows/bad.nika" | gate)"
expect "another subagent is not judged" 0 $? "$out$(cat "$ROOT/err")" ""
if [ -z "$out$(cat "$ROOT/err")" ]; then ok "another subagent draws no output"; else
  bad "another subagent draws output"
fi

out="$(payload "nika:nika-author" a1 t-good "Wrote workflows/good.nika, check clean." | gate)"
expect "a clean named workflow lets the author stop" 0 $? "$out" ""

out="$(payload "nika:nika-author" a2 t-bad "Wrote workflows/bad.nika." | gate)"
expect "findings send the author back" 2 $? "$(cat "$ROOT/err")" "NIKA-AUTH-006"
out="$(payload "nika:nika-author" a2 t-bad "Wrote workflows/bad.nika." | gate)"
expect "a second round is still sent back" 2 $? "$(cat "$ROOT/err")" "bad.nika does not pass"
out="$(payload "nika:nika-author" a2 t-bad "Wrote workflows/bad.nika." | gate)"
expect "after two rounds the author is let through with a warning" 0 $? "$out" "systemMessage"

out="$(payload "nika:nika-author" a3 t-good "All done, the file is ready." | gate)"
expect "a written workflow that is not named sends the author back" 2 $? "$(cat "$ROOT/err")" "names no such path"

out="$(payload "nika-author" a4 t-none "I only answered a question." | gate)"
expect "a turn that wrote and named nothing stops quietly" 0 $? "$out" ""

out="$(payload "nika:nika-author" a5 t-bad "Wrote workflows/bad.nika." | NIKA_BIN="$ROOT/bin/absent" CLAUDE_PLUGIN_DATA="$ROOT/data" python3 "$GATE" 2>"$ROOT/err")"
expect "a missing binary warns and lets the author stop" 0 $? "$out" "no nika binary"

out="$(printf 'not json at all' | gate)"
expect "a broken payload warns and lets the author stop" 0 $? "$out" "systemMessage"

out="$(printf '{"agent_type":"nika:nika-author","agent_id":"a6","cwd":"%s","agent_transcript_path":"%s/absent.jsonl","last_assistant_message":"Wrote workflows/good.nika."}' "$ROOT/proj" "$ROOT" | gate)"
expect "a transcript that is not there still checks the named file" 0 $? "$out" ""

if [ "$FAILS" -eq 0 ]; then echo "all green"; else
  echo "$FAILS failure(s)"
  exit 1
fi
