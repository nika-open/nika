- **`NIKA-SEC-009` credits a `mode: choice` gate only before an effect
  that tests its answer.** The lethal-trifecta check counted any blocking
  `nika:prompt` as a human gate, and the consent check (`NIKA-SEC-014`)
  reads only confirm gates. A choice question followed by an egress task
  that waits on it with `after:` alone therefore passed `nika check`,
  although that task runs whichever answer is picked, the one that
  declines included. A choice gate now counts before an egress task only
  when that task's own `when:` reads an exact `with:` binding of the
  gate's output and some listed choice makes the guard false (with no
  literal `choices:` list, the read alone counts). Otherwise the trifecta
  is refused as for no gate, and the finding names the gate and the task
  and gives both repairs: test the answer in `when:`, or use `mode:
  confirm`, whose « no » `NIKA-SEC-014` judges. A confirm gate is credited
  as before. Not judged: an answer tested by an upstream stage instead of
  the egress task itself, which is refused. `nika explain NIKA-SEC-009`
  states the rule.
