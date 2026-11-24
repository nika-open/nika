#!/usr/bin/env bash
# nika-report.test · the run report, built from a hand-written trace and a
# stub binary. No model, no run, nothing published.
#
# Laws under test: one HTML page and one Markdown twin land under the
# chosen folder · the page loads nothing from the network and runs no
# script · text a run recorded is escaped, never markup · a failed and a
# recovered task are named with their code · no trace store, a missing
# trace and a missing binary are said plainly with their own exit code.
#
# Usage: nika-report.test.sh  (exit 0 = all green)
set -uo pipefail

command -v python3 >/dev/null 2>&1 || {
  echo "FAIL nika-report.test: python3 is absent, so nothing was asserted" >&2
  exit 1
}
REPORT="$(cd "$(dirname "$0")/.." && pwd)/nika-report.py"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
FAILS=0
check() { # check <label> <haystack> <needle>
  if printf '%s' "$2" | grep -qF -- "$3"; then echo "ok   $1"; else
    echo "FAIL $1 — missing: $3"
    FAILS=$((FAILS + 1))
  fi
}
refuse() { # refuse <label> <haystack> <needle>
  if printf '%s' "$2" | grep -qF -- "$3"; then
    echo "FAIL $1 — unexpected: $3"
    FAILS=$((FAILS + 1))
  else echo "ok   $1"; fi
}
rc_is() { # rc_is <label> <want> <got>
  if [ "$2" -eq "$3" ]; then echo "ok   $1"; else
    echo "FAIL $1 — exit $3, wanted $2"
    FAILS=$((FAILS + 1))
  fi
}

P="$ROOT/proj"
mkdir -p "$P/.nika/traces" "$ROOT/empty" "$ROOT/bin"
TRACE=".nika/traces/2026-01-01T00-00-00Z-aaaa.ndjson"
{
  printf '{"kind":"workflow_started","timestamp":1767225600000000000,"fields":[{"key":"workflow","value":"weekly-digest"},{"key":"engine_version","value":"0.125.0"},{"key":"budget","value":"{\\"max_cost_usd\\":2.0}"}]}\n'
  printf '{"kind":"approval_decided","fields":[{"key":"task","value":"approve"},{"key":"question","value":"Post <b>this</b> to #general?"},{"key":"answer","value":"true"},{"key":"source","value":"resume"}]}\n'
  printf '{"kind":"task_completed","fields":[{"key":"task","value":"draft"},{"key":"note","value":"infer · acme/small"},{"key":"model","value":"acme/small"},{"key":"tokens_in","value":"120"},{"key":"tokens_out","value":"80"},{"key":"cost_usd","value":0.0125},{"key":"duration_ms","value":"900"}]}\n'
  printf 'a half-written line\n'
  printf '{"kind":"task_completed","fields":[{"key":"task","value":"save"},{"key":"note","value":"invoke · nika:write"},{"key":"output","value":"\\"./out/digest.md\\""}]}\n'
  printf '{"kind":"task_skipped","fields":[{"key":"task","value":"escalate"},{"key":"note","value":"when false"}]}\n'
  printf '{"kind":"task_failed","fields":[{"key":"task","value":"post"},{"key":"detail","value":"NIKA-BUILTIN-FETCH-001 · HTTP 503"}]}\n'
} >"$P/$TRACE"
cat >"$ROOT/bin/nika" <<STUB
#!/usr/bin/env bash
case "\$1 \$2" in
  "trace ls")
    if [ -d .nika/traces ]; then
      printf '%s\n' '{"traces":[{"path":"$TRACE","newest":true,"state":"failed","workflow":"weekly-digest"}]}'
    else printf '%s\n' '{"traces":[]}'; fi
    ;;
  "trace outputs") printf '%s\n' '{"state":"failed","settlement":{"elapsed_ms":1400,"spend":{"priced_calls":1,"unpriced_calls":0,"qualifier":"metered"},"tasks":{"ok":2,"failed":1,"recovered":1,"total":4}},"tasks":[{"id":"feed","status":"recovered","recovered_from":"NIKA-BUILTIN-FETCH-001","verb":"invoke · nika:fetch"},{"id":"draft","status":"ok","verb":"infer · acme/small"},{"id":"save","status":"ok","verb":"invoke · nika:write"},{"id":"post","status":"failed","error_code":"NIKA-BUILTIN-FETCH-001","error_message":"HTTP 503 <script>alert(1)</script>","verb":"invoke · nika:fetch"}]}' ;;
  *) exit 64 ;;
esac
STUB
chmod +x "$ROOT/bin/nika"
report() { (cd "$1" && shift && NIKA_BIN="$ROOT/bin/nika" python3 "$REPORT" "$@" 2>&1); }

out="$(report "$P")"
rc_is "the newest run is reported" 0 $?
check "the summary line names the result" "$out" "weekly-digest · failed · 4 tasks"
HTML="$P/out/nika-report-2026-01-01T00-00-00Z-aaaa.html"
MD="$P/out/nika-report-2026-01-01T00-00-00Z-aaaa.md"
if [ -s "$HTML" ] && [ -s "$MD" ]; then echo "ok   the page and its Markdown twin are written under out/"; else
  echo "FAIL the two files are not under out/"
  FAILS=$((FAILS + 1))
fi
page="$(cat "$HTML" 2>/dev/null)"
twin="$(cat "$MD" 2>/dev/null)"
check "the gate's question is on the page, escaped" "$page" "Post &lt;b&gt;this&lt;/b&gt; to #general?"
refuse "recorded text never becomes markup" "$page" "<script>alert(1)</script>"
refuse "the page runs no script" "$page" "<script"
refuse "the page loads nothing from the network" "$page" "http"
check "who answered the gate is said" "$twin" "answered \`true\` (resume)"
check "the written file is listed" "$twin" "./out/digest.md"
check "a failed task carries its code" "$twin" "\`post\` failed: NIKA-BUILTIN-FETCH-001"
check "a recovered task carries its code" "$twin" "\`feed\` recovered: NIKA-BUILTIN-FETCH-001"
check "a branch that was not taken is listed" "$twin" "\`escalate\` was skipped (when false)"
check "metered spend is the sum the trace holds, with the ceiling" "$twin" "\$0.0125 of a \$2.00 ceiling"
check "what the trace does not hold is said" "$twin" "Input billing by a provider is not in the trace"

out="$(report "$ROOT/empty")"
rc_is "an empty trace store is exit 3" 3 $?
check "and it is said plainly" "$out" "no run has been recorded"
out="$(report "$P" .nika/traces/absent.ndjson)"
rc_is "a trace that is not there is exit 3" 3 $?
out="$(cd "$P" && NIKA_BIN="$ROOT/bin/absent" PATH="/usr/bin:/bin" python3 "$REPORT" 2>&1)"
rc_is "a missing binary is exit 127" 127 $?
check "and it is said plainly" "$out" "nika is not installed"

if [ "$FAILS" -eq 0 ]; then echo "all green"; else
  echo "$FAILS failure(s)"
  exit 1
fi
