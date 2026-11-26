- **An `agent:` task on an ACP seat completes only when its turn ended.**
  Nika dropped the stop reason of a seat's turn, so a turn the model
  refused, one the adapter cut at its output limit and one that hit the
  adapter's own request limit all completed the task as if the agent had
  finished. `end_turn` still completes, and so does a seat backend that
  reports no stop reasons at all. `max_tokens` now fails NIKA-AGENT-006 like
  a cut final message. `refusal`, `max_turn_requests`, `cancelled` and a
  stop word Nika does not know fail NIKA-INFER-001 with the reason named. A
  turn that ends `cancelled` was called off by the adapter, on its own or
  after a refusal, or stopped on a request Nika could only cancel. A failure
  keeps the text received at `partial_output`, masked like every failed
  record, and records no token counts.
