- **A limit an `agent:` task declares on a seat is applied or refused
  before anything is sent.** On the `claude-code` seat, `max_turns` now
  reaches the session as its `maxTurns` option, beside the options that
  keep your Claude configuration out; a turn that reaches it fails
  `NIKA-INFER-001`, with stop reason `max_turn_requests` or the
  application's own words, and gets no schema repair.
  A task without `max_turns` still sends no limit: the default of 10
  turns is the native agent loop's. On every other seat `max_turns` is
  refused, and so is `temperature` on every seat, where both used to be
  dropped in silence: the run refuses the task before the seat is asked
  for a session (`NIKA-465`, the field named), and `nika check` names the
  same task as a blocker, one line per task, in the words it already used
  for `tools:`. Drop the field, or run the task on a seat or a model that
  applies it.
