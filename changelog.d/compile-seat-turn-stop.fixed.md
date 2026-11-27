- **An ACP authoring seat answers only from a turn that ended.** A
  harness seated by `nika compile --authoring-model <harness>/<model>`
  (every harness but `codex`) took each completed turn for an answer: a
  turn the model refused, one cut at its output limit, one stopped at the
  harness's own limit of model requests and one that was cancelled all
  handed what they had said to the compiler as if the turn had finished.
  Such a turn, and one that ends with any other stop word, now fails the
  authoring call with an `authoring_provider` diagnostic that names the
  harness and the stop reason, never the turn's text. Only `end_turn`
  answers. A harness that stops its turn `cancelled` after a tool ask the
  seat denied now fails the call too, where its text used to pass. The
  receipt still records the call with the tool asks it denied. The
  `codex/<model>` seat, which runs `codex exec` rather than ACP, is
  unchanged.
