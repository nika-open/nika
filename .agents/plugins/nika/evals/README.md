# Evals for the Nika plugin

Seven cases that measure what the plugin is for: Nika is offered on intent,
the engine reads the user's own sentence before anything is written, a
missing address is asked and never guessed, the verdict is said in plain
words, a question is never answered for the user, the plugin stays quiet
when it has nothing to add, and a new job is designed deep by default
(phases, branches, state between runs, validation, a run report, at most
one gate, no send nobody asked for).

Run from the plugin root (needs a signed-in Claude Code and `nika` on PATH):

    claude plugin eval . --scaffold --no-publish \
      --allow-tools "Bash(nika *)" "Bash(*/scripts/nika-*)" "Bash(echo *)" Write Skill

`claude plugin eval` also runs each case without the plugin, so the report
shows what the plugin adds. `--scaffold` is needed by `plain-verdict`, whose
fixture is a workflow refused on purpose, and by `deep-by-default`, whose
fixture is a folder of CI logs.

What the command line is for:

- **Only `--allow-tools` grants a tool in an eval.** The `allowed-tools` of a
  skill or a command does not count there. `"Bash(*/scripts/nika-*)"` lets the
  helper scripts run; `"Bash(echo *)"` lets a line such as
  `nika check f.nika; echo "exit=$?"` through, which is otherwise refused as a
  whole and scored as if the plugin had failed.
- **`--no-publish`** keeps the report on this machine. Without it the report
  is published to claude.ai.
- **`nika` must work with another home directory.** An eval run replaces
  `HOME` with a temporary folder. A launcher that finds the real binary
  through `$HOME` then answers "no nika binary to run", and the case measures
  that failure instead of the plugin. Put the real release binary in a folder
  that is first on `PATH` for the eval command, and confirm with a one-run
  probe case (`runs: 1` in its `case.yaml`) that `nika --version` answers.
- **The MCP oracle is not started** unless you add a mock under
  `evals/mocks/` or pass `--allow-real-servers`. The seven cases do not need it.
- Add `--max-cost-usd <n>` for a hard ceiling. `--case <name> --ablation none
  --keep-temp` reruns one case with the plugin only and keeps its trace.

Results are written to `evals/results/`, which is not committed.
