- **A seat's record says when its adapter runs a release other than the one
  its boundary was measured on.** What the codex and claude-code seats do
  without asking was measured on one adapter release each (codex-acp 1.13.1,
  claude-agent-acp 0.81.1). On another release that the seat still accepts,
  Nika applies the same rule: it selects and reads back the same mode,
  refuses before the prompt when the adapter does not keep it, and still
  reads a Codex command ask as leaving its sandbox. The `permit_checked`
  record of that session now adds one sentence after the measured text,
  naming the release measured and the one the session runs; a release that
  does not read as `MAJOR.MINOR.PATCH` is never taken for the measured one.
  A release outside the accepted range is refused, as before, and the
  refusal now names the range and the release to install. On these two
  seats a model the task names is selected only through the session's model
  option: a model only the adapter's older model list names refuses before
  the prompt instead of being sent through `session/set_model`, and an
  access offer reports it as not offered. `initialize` now names the client
  (`clientInfo`: `nika` and the engine's version).
