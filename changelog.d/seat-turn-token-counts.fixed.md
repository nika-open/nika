- **An ACP seat task records the token counts its adapter reported and holds
  `max_tokens_total` to them.** The token counts of the adapter's
  `session/prompt` result now ride the task (`tokens`, `tokens_in`,
  `tokens_out`, with `usage_source: adapter_prompt_result`), weighed like a
  native turn's, so `max_tokens_total` refuses a repair when the first turn
  reached or passed it, with NIKA-AGENT-002; counts too large to add up, in
  one turn or across a turn and its repair, are unknown. An answer with no
  counts (Codex sends `usage: null` on a turn that failed) leaves `tokens`
  unknown instead of 0, says `usage_source: unreported`, and marks a
  declared ceiling `token_ceiling: unjudged`.
