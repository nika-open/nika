- **A refused request of an ACP seat selects the adapter's one-time
  refusal.** Nika answered a request you refused at the gate (`--answer
  <task>=false`) with `cancelled`, the protocol's word for calling the turn
  off. It now selects the first `reject_once` option the adapter offers, as
  the conversation path already did, and never a remembered refusal. What
  follows is the adapter's: some options let the agent go on without the
  tool, where the refusal could end the turn before, and others still end it
  (in the codex-acp 1.13.1 source, as read, a file change offers only "No,
  and tell Codex what to do differently"). An adapter that offers no
  one-time refusal still hears `cancelled`, and so does a request the engine
  never answered.
