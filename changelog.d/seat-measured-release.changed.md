- **A seat's record says when its adapter runs a release other than the one
  its boundary was measured on.** What each measured row (`codex`,
  `claude-code` and `gemini-cli`) does without asking was measured on one
  adapter release (codex-acp 1.13.1, claude-agent-acp 0.81.1, gemini-cli
  0.62.0). On another release that the seat still accepts, Nika applies the
  same rule: it selects and reads back the same mode, refuses before the
  prompt when the adapter does not keep it, and still reads a Codex command
  ask as leaving its sandbox. The `permit_checked` record of that session
  now adds one sentence after the measured text: "What the harness does
  without asking was measured on {measured release}; this session's adapter
  reports release `{its release}`, on which it was not measured." A refusal
  of the mode before the prompt ends with the same sentence. A release that
  does not read as `MAJOR.MINOR.PATCH` is never taken for the measured one,
  and an empty one is read as no release. On the `codex` and `claude-code`
  seats, which judge the release reported at the handshake, a release
  outside the accepted range is refused as before, and the refusal now adds
  the release to install, the one the boundary was measured on; the
  `gemini-cli` seat keeps its own refusal of a `gemini --version` outside
  its range. On these two seats a model named for a run or a Session
  conversation is selected only through the session's model option: a model
  only the adapter's older model list names refuses before the prompt
  instead of being sent through `session/set_model`, on every release the
  seat accepts, and an access offer reports it as not offered.
  `initialize` now names the client (`clientInfo`: `nika` and the engine's
  version).
