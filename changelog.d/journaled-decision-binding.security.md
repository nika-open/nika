- **A decision journaled for a gate no longer carries over to a task
  edited after it.** A resume served a decided `nika:prompt` gate from its
  trace without looking at the tasks behind it, so on `nika run --resume
  <trace> --answer <gate>=<answer>` a task behind an earlier, already
  decided gate ran in its edited form under that earlier decision. The
  record of a decided gate now carries the hash of the content its
  decision bound (`approval_shown_hash`, beside `def_hash`): the question,
  the gate as written and the definition of every task downstream of it
  up to and including the next gate, the content a paused answer binds.
  Before serving that decision a resume recomputes the content from the
  files it runs. A decision over other content is refused with
  `approval.content_mismatch` (`NIKA-SEC-010`): the gate fails and no
  task that waits on it starts. A decision behind which the engine holds
  no identity for a task is refused with `approval.content_unhashable`.
  The refusal names the gate, and adding `--from <gate>` to the resume
  asks it again. A trace written by an earlier engine journals no such
  hash, so its decisions are refused with `approval.content_mismatch`
  too. A decision over unchanged content is served as before.
