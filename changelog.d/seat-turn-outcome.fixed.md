- **An `agent:` task on an ACP seat reads how its turn ended and what the
  adapter counted.** Nika dropped the stop reason of a seat's turn, so a
  turn the model refused, one the adapter cut at its output limit and one
  that hit the adapter's own request limit all completed the task as if the
  agent had finished. `end_turn` still completes, and so does a seat backend
  that reports no stop reasons at all. `max_tokens` now fails NIKA-AGENT-006
  like a cut final message. `refusal`, `max_turn_requests`, `cancelled` and
  a stop word Nika does not know fail NIKA-INFER-001 with the reason named:
  Nika never cancels the turn of a task, so a turn that ends `cancelled` was
  cut by the adapter. A failure keeps the text received at `partial_output`,
  masked like every failed record, and records no token counts. The
  `schema:` and its repair judge the turn's last message: a tool call always
  starts a new message, and so does a chunk whose `messageId` differs from
  the current message's or that carries an id after chunks without one, so
  narration before a tool call is no longer parsed as the answer. A failure
  keeps every message of the turn; a success of several messages keeps the
  whole text in the trace (`seat_transcript`, its first 64 KiB, with the
  run's secrets masked). One turn may hold 4 MiB of text. The token counts
  of the adapter's `session/prompt` result now ride the task (`tokens`,
  `tokens_in`, `tokens_out`, with `usage_source: adapter_prompt_result`),
  weighed like a native turn's, so `max_tokens_total` refuses a repair when
  the first turn reached or passed it, with NIKA-AGENT-002; counts too large
  to add up are unknown. A seat that reports no counts (Codex sends `usage:
  null`) leaves `tokens` unknown instead of 0, says `usage_source:
  unreported`, and marks a declared ceiling `token_ceiling: unjudged`. The
  context occupancy of the latest `usage_update` rides as `context_used` and
  `context_size`, and Claude's session cost as `cost_estimate`, the
  adapter's own unbilled estimate with its currency: it never becomes
  `cost_usd` or reaches a cost ceiling, an update without a cost clears it,
  and a cost whose currency is not three capital letters is dropped. The
  model Claude reports having answered with is the task's `model_reported`
  (source `adapter_usage_update`) and the receipt's responder, beside the
  configured model; under an exact selection, a reported model that is
  demonstrably another one (another model line, version or release date)
  refuses the answer with NIKA-1805, as a model moved during the turn does,
  and a spelling Nika cannot place is only recorded. A Codex prompt that
  fails on an exhausted quota now shows Codex's own words beside the
  JSON-RPC code.
