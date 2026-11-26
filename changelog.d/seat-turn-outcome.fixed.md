- **An `agent:` task on an ACP seat reads how its turn ended and what the
  adapter counted.** Nika dropped the stop reason of a seat's turn, so a
  turn the model refused, one the adapter cut at its output limit and one
  that hit the adapter's own request limit all completed the task as if
  the agent had finished. `end_turn` still completes, and `cancelled` keeps
  its meaning; `max_tokens` now fails NIKA-AGENT-006 like a cut final
  message, and `refusal`, `max_turn_requests` and a stop word Nika does not
  know fail NIKA-INFER-001 with the reason named and the text received kept
  at `partial_output`, masked like every failed record. The `schema:` and
  its repair judge the turn's last message: a change of `messageId`, or a
  tool call between chunks that carry none, starts a new message, so
  narration before a tool call is no longer parsed as the answer. A failure
  keeps every message of the turn and a success keeps the whole text in
  the trace (`seat_transcript`); one turn may hold 4 MiB of text. The
  token counts of the adapter's `session/prompt` result now ride the task
  (`tokens`, `tokens_in`, `tokens_out`, with `usage_source:
  adapter_prompt_result`), weighed like a native turn's, so
  `max_tokens_total` refuses a repair the first turn already exceeded with
  NIKA-AGENT-002. A seat that reports no counts (Codex sends `usage: null`)
  leaves `tokens` unknown instead of 0, says `usage_source: unreported`,
  and marks a declared ceiling `token_ceiling: unjudged`. The context
  occupancy of the latest `usage_update` rides as `context_used` and
  `context_size`, and Claude's session cost as `cost_estimate`, the
  adapter's own unbilled estimate with its currency: it never becomes
  `cost_usd` or reaches a cost ceiling, and an update without a cost clears
  it. The model Claude reports having answered with is the task's
  `model_reported` (source `adapter_usage_update`) and the receipt's
  responder, beside the configured model; under an exact selection, a
  reported model that is not the confirmed one refuses the answer with
  NIKA-1805, as a model moved during the turn does. A Codex prompt that
  fails on an exhausted quota now shows Codex's own words beside the
  JSON-RPC code.
