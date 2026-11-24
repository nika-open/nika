- **`nika check` names a guard that untrusted content steers.** With
  private read, untrusted ingress and external egress all permitted, a task
  whose `when:` reads the output of a fetch, an exec, an MCP result or a
  model answer that saw one, and that is an egress task or has one
  downstream, now carries the advisory hint `branch-steering` (`nika explain
  branch-steering`). `NIKA-SEC-009` judges content that reaches a task's
  arguments, and a guard is not an argument: the content can pick whether
  the branch runs without ever entering a payload. The hint names the task,
  the task whose output the guard reads and the ingress the content came
  from, and prints the repair: bind a confirm gate's answer and test it
  first, so the content can only cancel what a person approved. Nothing is
  refused and `is_clean` is unchanged. Not judged in this version: a guard
  that reads only `inputs.*`, a guard inside a child workflow, a `for_each:`
  collection, a status read. `nika_cap::TrifectaVerdict` gains `armed`,
  whether the three legs held.
