#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# nika-facts — the names an author needs, read from the binary just now.
#
# A skill written last month names last month's builtins and model routes.
# The authoring and running skills run this when they load, so the names in
# front of the agent are the ones this binary serves. Short on purpose: it
# is injected into context. Prints and exits 0 whatever is missing; a skill
# that loads with the injection disabled reads the same names with the
# commands printed in its own first lines.
#
# Usage: nika-facts.sh [project-dir]
set -u
here="$(cd "$(dirname "$0")" && pwd)"
project="${1:-$PWD}"
if [ -x "$here/../bin/nika-kit" ]; then
  bin="$("$here/../bin/nika-kit" --path "$project" 2>/dev/null)"
else
  bin="${NIKA_BIN:-$(command -v nika 2>/dev/null)}"
fi
if [ -z "${bin:-}" ] || ! command -v "$bin" >/dev/null 2>&1; then
  echo "Nika facts: no nika binary was found, so no name was read. Install it (curl -LsSf https://nika.sh/install.sh | sh), then read names from \`nika catalog --tools\`, \`nika catalog\` and \`nika spec --canon\`."
  exit 0
fi

echo "Nika facts, read from the binary just now. Trust these names over memory."
echo "- binary: $bin · $("$bin" --version 2>/dev/null | head -n 1)"
builtins="$("$bin" spec --canon 2>/dev/null | awk '
  /^builtins:/ { inside = 1 }
  inside && /^  items:/ { items = 1; next }
  inside && items && /^    - / { printf "%snika:%s", (n++ ? ", " : ""), $2; next }
  inside && items { exit }')"
echo "- builtins: ${builtins:-not listed by \`nika spec --canon\` here; read \`nika catalog --tools\`}"

if command -v python3 >/dev/null 2>&1; then
  # shellcheck disable=SC2016
  "$bin" catalog --json 2>/dev/null | python3 -c 'import json, sys
try:
    rows = [p for p in json.load(sys.stdin).get("providers", []) if p.get("resolves")]
except Exception:
    rows = []
routes = ["%s/%s%s" % (p.get("id"), p.get("cheap_model") or p.get("default_model"), " (local)" if p.get("local") else "") for p in rows]
print("- model routes, one per provider this binary resolves (its cheaper model; `nika catalog` lists every model): " + ("; ".join(routes) if routes else "not listed here; read `nika catalog`"))'
  waiting="$("$here/nika-pending.py" "$project" --json 2>/dev/null | python3 -c 'import json, sys
try:
    print(len(json.load(sys.stdin).get("waiting", [])))
except Exception:
    print("unknown")' 2>/dev/null)"
  echo "- runs waiting for an answer in $project: ${waiting:-unknown}"
else
  echo "- model routes and waiting runs: python3 is absent; read \`nika catalog\` and \`nika trace ls\`"
fi
if "$bin" catalog --help 2>/dev/null | grep -q -- '--mcp'; then
  echo "- MCP shelf: $("$bin" catalog --mcp 2>/dev/null | head -n 20 | tr '\n' ' ' | cut -c1-900)"
else
  echo "- MCP shelf: this binary prints no list of it; take a server's launch command from its own documentation"
fi
exit 0
