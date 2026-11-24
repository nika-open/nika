#!/usr/bin/env bash
# workflow-scripts.test · the workflow scripts keep the shape their runner needs.
#
# A workflow script is run by the host, not by this repository, so nothing
# here executes one. What can be proven without a session: `meta` is a plain
# literal on the first line and names the file, every phase started in the
# body is declared in `meta` (titles are matched exactly), the body reads no
# clock, no randomness and no file (a resumed run would diverge), and the
# file parses once wrapped the way the runner wraps it.
#
# Usage: workflow-scripts.test.sh  (exit 0 = all green)
set -uo pipefail

KIT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
FAILS=0
bad() {
  echo "FAIL $1"
  FAILS=$((FAILS + 1))
}

count=0
for script in "$KIT"/workflows/*.js; do
  [ -f "$script" ] || continue
  count=$((count + 1))
  name="$(basename "$script" .js)"
  if [ "$(head -n 1 "$script")" = "export const meta = {" ]; then echo "ok   $name starts with its meta literal"; else
    bad "$name does not start with « export const meta = { »"
  fi
  grep -qF "name: '$name'," "$script" || bad "$name: meta.name is not the file name"
  for banned in 'import(' 'require(' 'Date.now' 'new Date(' 'Math.random' 'process.' 'readFile' 'writeFile'; do
    if grep -qF -- "$banned" "$script"; then bad "$name uses $banned, which a workflow script cannot"; fi
  done
  declared="$(grep -o "{ title: '[^']*'" "$script" | sed "s/.*'\(.*\)'/\1/" | sort -u)"
  started="$({
    grep -o "^phase('[^']*')" "$script" | sed "s/.*('\(.*\)')/\1/"
    grep -o "phase: '[^']*'" "$script" | sed "s/.*'\(.*\)'/\1/"
  } | sort -u)"
  if [ -n "$declared" ] && [ "$declared" = "$started" ]; then echo "ok   $name declares the phases it starts"; else
    bad "$name: phases declared in meta and phases started differ"
  fi
  if command -v node >/dev/null 2>&1; then
    {
      echo 'async function workflowBody(args, agent, parallel, pipeline, phase, log, budget, workflow) {'
      sed '1s/^export const meta/const meta/' "$script"
      echo '}'
    } >"$TMP/$name.mjs"
    if node --check "$TMP/$name.mjs" 2>"$TMP/err"; then echo "ok   $name parses"; else
      bad "$name does not parse: $(head -n 3 "$TMP/err" | tr '\n' ' ')"
    fi
  else
    echo "skip $name syntax: no node on this machine"
  fi
done
[ "$count" -ge 3 ] || bad "expected the three workflow scripts, found $count"

if [ "$FAILS" -eq 0 ]; then echo "all green"; else
  echo "$FAILS failure(s)"
  exit 1
fi
