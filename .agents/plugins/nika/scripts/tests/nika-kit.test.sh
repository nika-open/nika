#!/usr/bin/env bash
# nika-kit.test · the resolver names the binary the work targets, says why,
# and never falls back in silence. Stub binaries only: nothing is run.
#
# The six situations are the ones check-on-edit-oracle.test.sh plays
# against the edit hook, so the two cannot disagree without one going red.
#
# Usage: nika-kit.test.sh  (exit 0 = all green)
set -uo pipefail
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY

KIT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/bin/nika-kit"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
fails=0
bad() {
  printf 'FAIL %s\n' "$1"
  fails=$((fails + 1))
}

stub() { # stub <dir> <label> [feature…]
  mkdir -p "$1"
  cat >"$1/nika" <<STUB
#!/usr/bin/env bash
case "\$1 \${2:-}" in
  "--version "*) echo "nika 0.0.0 ($2)" ;;
  "help consent") case " ${*:3} " in *" consent "*) exit 0 ;; *) exit 2 ;; esac ;;
  "catalog --help") case " ${*:3} " in *" mcp-shelf "*) echo "      --mcp   list the shelf" ;; *) echo "      --json" ;; esac ;;
  *) exit 64 ;;
esac
STUB
  chmod +x "$1/nika"
}

stub "$ROOT/explicit" explicit consent mcp-shelf
stub "$ROOT/session-target/debug" session-target
stub "$ROOT/path-bin" path-bin
ENGINE="$ROOT/engine"
mkdir -p "$ENGINE/crates/nika-cli" "$ENGINE/flows"
: >"$ENGINE/crates/nika-cli/Cargo.toml"
stub "$ENGINE/target/debug" tree-build
git -C "$ENGINE" init -q
PROJECT="$ROOT/project"
mkdir -p "$PROJECT"
git -C "$PROJECT" init -q

kit() { env -u NIKA_BIN -u CARGO_TARGET_DIR PATH="$ROOT/path-bin:/usr/bin:/bin" "$@"; }
picks() { # picks <label> <location> <wanted stub> [VAR=value…]
  local label="$1" where="$2" want="$3" out
  shift 3
  out="$(kit "$@" bash "$KIT" "$where" 2>&1)"
  if printf '%s' "$out" | grep -qF "($want)" && printf '%s' "$out" | grep -q '^why: .'; then
    printf 'ok   %s\n' "$label"
  else bad "$label: wanted $want with a reason, got: $out"; fi
}

picks "NIKA_BIN wins everywhere" "$ENGINE/flows" explicit \
  NIKA_BIN="$ROOT/explicit/nika" CARGO_TARGET_DIR="$ROOT/session-target"
picks "inside the engine, the session target folder" "$ENGINE/flows" session-target \
  CARGO_TARGET_DIR="$ROOT/session-target"
picks "inside the engine, no session folder: the tree build" "$ENGINE/flows" tree-build
picks "a session folder with no build falls back to the tree build" "$ENGINE/flows" tree-build \
  CARGO_TARGET_DIR="$ROOT/empty-target"
picks "outside the engine, the session folder is not used" "$PROJECT" path-bin \
  CARGO_TARGET_DIR="$ROOT/session-target"
picks "outside the engine, NIKA_BIN names it" "$PROJECT" explicit NIKA_BIN="$ROOT/explicit/nika"

out="$(kit NIKA_BIN="$ROOT/absent/nika" bash "$KIT" --path "$PROJECT" 2>&1)"
rc=$?
if [ "$rc" -eq 1 ] && printf '%s' "$out" | grep -qF "not an executable" && ! printf '%s' "$out" | grep -qF "path-bin"; then
  echo "ok   a broken pin is an error, not a fall back to the PATH"
else bad "a broken pin: exit $rc, $out"; fi

out="$(env -u NIKA_BIN -u CARGO_TARGET_DIR PATH="/usr/bin:/bin" bash "$KIT" --path "$PROJECT" 2>&1)"
rc=$?
if [ "$rc" -eq 1 ] && printf '%s' "$out" | grep -qF "no nika binary"; then
  echo "ok   no binary anywhere says so and exits 1"
else bad "no binary: exit $rc, $out"; fi

out="$(kit bash "$KIT" --path "$PROJECT")"
if [ "$out" = "$ROOT/path-bin/nika" ]; then echo "ok   --path prints the path alone"; else
  bad "--path printed: $out"
fi

has() { # has <label> <feature> <wanted rc> [VAR=value…]
  local label="$1" feature="$2" want="$3"
  shift 3
  kit "$@" bash "$KIT" --has "$feature" "$PROJECT" >/dev/null 2>&1
  local rc=$?
  if [ "$rc" -eq "$want" ]; then echo "ok   $label"; else bad "$label: exit $rc, wanted $want"; fi
}
has "a binary with a consent verb has it" consent 0 NIKA_BIN="$ROOT/explicit/nika"
has "a binary without one does not" consent 1
has "a binary that lists the MCP shelf has it" mcp-shelf 0 NIKA_BIN="$ROOT/explicit/nika"
has "a binary that does not list it does not" mcp-shelf 1
has "no binary is neither yes nor no" consent 2 NIKA_BIN="$ROOT/absent/nika"

if [ "$fails" -eq 0 ]; then echo "all green"; else
  echo "$fails failure(s)"
  exit 1
fi
