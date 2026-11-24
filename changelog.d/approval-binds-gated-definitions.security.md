- **A paused approval answer no longer carries over to a task edited after
  the question.** The ticket of a `nika:prompt` gate bound the question
  and, for each effectful task behind the gate, its name and effect
  classes only. A gated `nika:write` rewritten while the run waited, a
  downstream `infer` whose prompt changed or a child workflow swapped on
  disk therefore still ran on `nika run --resume <trace> --answer
  <gate>=<answer>`. The ticket now binds the gate as written and the
  definition of every task downstream of it, with or without an effect
  class, up to and including the next gate. A definition is named by the
  hash the trace already journals for the task (`def_hash`), so it covers
  an inference's effective model and access, an agent's skill texts and a
  child workflow's source closure, and a comment or a layout edit leaves it
  as it was. An answer that meets another definition is refused with
  `approval.content_mismatch` (`NIKA-SEC-010`): the gate fails and no task
  that waits on its answer starts. An answer behind which the engine holds
  no identity for a task is refused with `approval.content_unhashable`. A
  run paused by an earlier engine holds a ticket without definitions: its
  answer is refused with `approval.content_mismatch` too, and resuming
  that run without `--answer` asks again. Still not bound: a value a gated
  task reads from elsewhere (a constant, an input, the output of a task
  outside the gate's closure) unless the question shows it.
