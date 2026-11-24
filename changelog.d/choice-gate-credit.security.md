- **`NIKA-SEC-009` credits only a confirm gate by its position alone.** The
  lethal-trifecta check counted any blocking `nika:prompt` as a human gate,
  and the consent check (`NIKA-SEC-014`) reads only confirm gates. A `mode:
  choice` or `mode: input` question followed by an egress task that waits on
  it with `after:` alone therefore passed `nika check`, although that task
  runs whatever the answer, the one that declines included. Such a gate now
  counts before an egress task only when that task's own `when:` tests the
  answer: it reads an exact `with:` binding of the gate's output and some
  listed choice makes the guard false (for an input or a templated
  `choices:` list, the read alone counts), or it reads the output of a
  `nika:jq` stage whose `with:` binds the answer, directly or through
  further such stages. Otherwise the trifecta is refused as for no gate, and
  the finding names the gate and the task and gives both repairs: test the
  answer in `when:`, or use `mode: confirm`, whose « no » `NIKA-SEC-014`
  judges. A confirm gate is credited as before. Not judged: what the jq
  program computes from the answer it binds. An answer tested through any
  other stage (a model, a subprocess, another tool) is refused. The
  `headless-prompt` hint on a choice gate now suggests one of its choices
  instead of `--answer <id>=false`, and `nika explain NIKA-SEC-009` states
  the rule.
