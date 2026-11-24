#!/usr/bin/env bash
# check-on-edit-oracle.test.sh: which binary the edit hook judges with.
#
# The order under test: NIKA_BIN when set; else, inside the engine tree,
# the session's CARGO_TARGET_DIR build, then the tree's target/debug build;
# else the PATH's `nika`. The session folder never judges a file outside
# the engine tree: there it may hold another project's build.
#
# Stub binaries stand in for the real ones, so this runs with no engine
# build. Each stub refuses every file and names itself in its finding; the
# hook's stderr then says which one answered.
#
# Usage: check-on-edit-oracle.test.sh  (exit 0 = all green)
set -uo pipefail
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$HERE/../check-on-edit.sh"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
fails=0

# stub <dir> <name>: an executable `nika` that refuses every check as <name>
stub() {
  mkdir -p "$1"
  cat >"$1/nika" <<EOF
#!/usr/bin/env bash
case "\$1" in
  --version) echo "nika 0.0.0 ($2)" ;;
  *) echo "finding from $2"; exit 2 ;;
esac
EOF
  chmod +x "$1/nika"
}

stub "$ROOT/explicit" explicit
stub "$ROOT/session-target/debug" session-target
stub "$ROOT/path-bin" path-bin

# The engine tree: a git repository holding crates/nika-cli and a tree build.
ENGINE="$ROOT/engine"
mkdir -p "$ENGINE/crates/nika-cli" "$ENGINE/flows"
: >"$ENGINE/crates/nika-cli/Cargo.toml"
stub "$ENGINE/target/debug" tree-build
git -C "$ENGINE" init -q
printf 'nika: flow\npermits: {}\ntasks: {}\n' >"$ENGINE/flows/flow.nika"

# A project outside the engine: a git repository with no crates/nika-cli.
PROJECT="$ROOT/project"
mkdir -p "$PROJECT"
git -C "$PROJECT" init -q
printf 'nika: flow\npermits: {}\ntasks: {}\n' >"$PROJECT/flow.nika"

# judge <label> <file> <want> [env assignments...]: the hook's oracle line names <want>
judge() {
  local label="$1" file="$2" want="$3"
  shift 3
  local payload out
  payload="$(printf '{"hook_event_name":"PostToolUse","tool_input":{"file_path":"%s"}}' "$file")"
  out="$(printf '%s' "$payload" | env -u NIKA_BIN -u CARGO_TARGET_DIR \
    PATH="$ROOT/path-bin:/usr/bin:/bin" "$@" bash "$HOOK" 2>&1)"
  if printf '%s' "$out" | grep -qF "finding from $want"; then
    printf 'ok   %s\n' "$label"
  else
    printf 'FAIL %s: wanted the %s judge, got:\n%s\n' "$label" "$want" "$out"
    fails=$((fails + 1))
  fi
}

judge "NIKA_BIN wins everywhere" "$ENGINE/flows/flow.nika" explicit \
  NIKA_BIN="$ROOT/explicit/nika" CARGO_TARGET_DIR="$ROOT/session-target"
judge "inside the engine, the session target folder judges" "$ENGINE/flows/flow.nika" session-target \
  CARGO_TARGET_DIR="$ROOT/session-target"
judge "inside the engine, no session folder: the tree build judges" "$ENGINE/flows/flow.nika" tree-build
judge "a session folder with no build falls back to the tree build" "$ENGINE/flows/flow.nika" tree-build \
  CARGO_TARGET_DIR="$ROOT/empty-target"
judge "outside the engine, the session folder never judges" "$PROJECT/flow.nika" path-bin \
  CARGO_TARGET_DIR="$ROOT/session-target"
judge "outside the engine, NIKA_BIN names the judge" "$PROJECT/flow.nika" explicit \
  NIKA_BIN="$ROOT/explicit/nika"

if [ "$fails" -eq 0 ]; then
  echo "all green"
else
  echo "$fails failure(s)"
  exit 1
fi
