#!/usr/bin/env bash
# evals.test · the eval suite keeps a shape its runner can load.
#
# Nothing here runs a case: that needs a signed-in session and costs money.
# What can be proven for free: every case has a prompt with its frontmatter
# and at least one grader, every grader is of a known type with the keys that
# type needs, a case that declares a fixture ships it, the six cases of the
# loop keep their free graders first (at least two, and at most one grader
# judged by a model), and the README names the number of cases the folder
# holds and every one of the six.
#
# Usage: evals.test.sh  (exit 0 = all green)
set -uo pipefail

EVALS="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/evals"
FAILS=0
bad() {
  echo "FAIL $1"
  FAILS=$((FAILS + 1))
}
field() { # field <file> <key>: the value of a frontmatter key, or nothing
  sed -n '2,/^---$/p' "$1" | sed -n "s/^$2:[[:space:]]*//p" | head -n 1
}

cases=0
for prompt in "$EVALS"/*/prompt.md; do
  dir="$(dirname "$prompt")"
  name="$(basename "$dir")"
  cases=$((cases + 1))
  [ "$(head -n 1 "$prompt")" = "---" ] || bad "$name: prompt.md has no frontmatter"
  [ -n "$(field "$prompt" max_turns)" ] || bad "$name: prompt.md sets no max_turns"
  [ -n "$(field "$prompt" allowed_tools)" ] || bad "$name: prompt.md sets no allowed_tools"
  if [ -f "$dir/case.yaml" ]; then
    grep -q "^name: $name\$" "$dir/case.yaml" || bad "$name: case.yaml names another case"
    script="$(sed -n 's/^[[:space:]]*scaffold_script:[[:space:]]*//p' "$dir/case.yaml")"
    if [ -n "$script" ] && [ ! -f "$dir/$script" ]; then bad "$name: case.yaml names a fixture it does not ship"; fi
  fi
  graders=0
  judged=0
  for grader in "$dir"/graders/*.md; do
    [ -f "$grader" ] || continue
    graders=$((graders + 1))
    kind="$(field "$grader" type)"
    case "$kind" in
      llm)
        judged=$((judged + 1))
        if ! grep -q '^PASS ' "$grader" || ! grep -q '^FAIL ' "$grader"; then
          bad "$name/$(basename "$grader"): a judged grader says PASS if and FAIL if"
        fi
        ;;
      tool_used)
        if [ -z "$(field "$grader" tool)" ] || [ -z "$(field "$grader" input_match)" ]; then
          bad "$name/$(basename "$grader"): tool_used needs tool and input_match"
        fi
        ;;
      regex) [ -n "$(field "$grader" pattern)" ] || bad "$name/$(basename "$grader"): regex needs a pattern" ;;
      file_exists) [ -n "$(field "$grader" path)" ] || bad "$name/$(basename "$grader"): file_exists needs a path" ;;
      *) bad "$name/$(basename "$grader"): unknown grader type « $kind »" ;;
    esac
  done
  [ "$graders" -ge 1 ] || bad "$name has no grader"
  case "$name" in
    reviews-before-handover | surfaces-gate-from-background-run | routes-cheap-for-mechanical-step | proposes-tool-from-catalog | improves-from-trace | never-creates-consent)
      [ "$judged" -le 1 ] || bad "$name has $judged graders judged by a model, at most one is allowed"
      [ $((graders - judged)) -ge 2 ] || bad "$name needs at least two free graders"
      [ -f "$dir/fixture.sh" ] || bad "$name ships no fixture"
      grep -q "\`$name\`" "$EVALS/README.md" || bad "the README does not name $name"
      echo "ok   $name: $((graders - judged)) free graders, $judged judged"
      ;;
  esac
done

words="zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen"
said="$(echo "$words" | cut -d' ' -f$((cases + 1)))"
if grep -qi "^$said cases" "$EVALS/README.md"; then echo "ok   the README counts the $cases cases the folder holds"; else
  bad "the folder holds $cases cases and the README does not open with « $said cases »"
fi

if [ "$FAILS" -eq 0 ]; then echo "all green"; else
  echo "$FAILS failure(s)"
  exit 1
fi
