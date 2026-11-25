- **An `agent:` task on a seat gets one repair of a final message that
  misses its `schema:`.** A seat's final message that did not validate
  failed the task with NIKA-INFER-002 at once, and the trace kept only the
  first violation: an agent that worked for minutes and answered with one
  key missing lost the whole run (since 0.124.0). The message now gets one
  repair prompt in the same session, which still holds the agent's work: it
  lists every violation and the schema, asks for the final message only,
  and shows in the trace as an `agent_nudge` with reason `schema_repair`.
  The repair must fit the task's `max_tokens_total` (a budget already met
  fails NIKA-AGENT-002). A repaired message that misses again, a session
  the application closed with its final message, or a repair turn that
  never answers (the application exits, errors or goes silent) fails
  NIKA-INFER-002 with the first message kept, and
  `task_failed` now carries the rejected message (`partial_output`) and
  every violation (up to 32, then a count). A violation no longer copies the
  value it judged, which for a type miss at the root was the whole message:
  it names the place and the rule (`/tags: value is not of type "array"`),
  in the repair prompt, the error message and the API agent's re-ask alike.
  The trace keeps the first 64 KiB of the message, cut only after the run's
  secrets in it are masked; `on_error:` reads the message whole.
  NIKA-AGENT-001 and NIKA-AGENT-002 keep their
  `partial_output` too, as spec 02 says. `on_codes: [NIKA-INFER-002]` in a
  `retry:` block replays the task on a new session. `nika explain` no
  longer closes a code only a run raises (NIKA-INFER-002, the agent budget
  codes, timeouts, cancellations and the others) with « `nika check`
  catches this before a run ever starts »: it says the run raises it, and
  says which part check does catch for NIKA-INFER-004 and NIKA-VAR-006.
