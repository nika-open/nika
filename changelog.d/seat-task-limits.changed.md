- **A limit an `agent:` task declares on a seat is sent to the seat or
  refused before anything is sent.** On the `claude-code` seat, `max_turns`
  is now sent to the session as its `maxTurns` option, beside the options
  that keep your Claude configuration out; Claude Code counts the turns,
  not Nika. Measured with claude-agent-acp 0.81.1, the session stops at the
  limit and reports the turn as failed, in its own words (`Reached maximum
  number of turns (N)`): the task fails `NIKA-1805` when the workflow
  declares `run.access`, and `NIKA-INFER-001` when it does not. A session
  that ends the turn with the stop reason `max_turn_requests` instead fails
  `NIKA-INFER-001` either way. Such a first turn gets no schema repair.
  A task without `max_turns` still sends no limit: the default of 10 turns
  is the native agent loop's. On every other seat `max_turns` is refused,
  and so is `temperature` on every seat, where both used to be dropped in
  silence: the run refuses the task before the seat is asked for a session
  (`NIKA-465`, the field named), and `nika check` names the same task as a
  blocker, one line per task, in the words it already used for `tools:`.
  Drop `max_turns:` or run the task on the `claude-code` seat or an API or
  local model; drop `temperature:` or run the task on an API or local
  model.
