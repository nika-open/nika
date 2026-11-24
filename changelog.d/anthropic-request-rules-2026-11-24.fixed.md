- **The Anthropic wire refuses two more requests the current Claude models
  answer with HTTP 400, before any byte is sent.** No workflow reaches them
  today: the engine never sets a forced tool choice or a provider extra
  `thinking.type`, so they guard a program that builds requests with
  `nika-providers` directly. A forced tool choice (`any` or a named tool) is
  refused on Claude Opus 5.5, Sonnet 5.5, Fable 5.2, Mythos 5.2, Fable 5.1
  and Mythos 5.1, with the repair (`auto`, or a structured output); Fable 5,
  Haiku 5.5, Opus 5 and the earlier models accept it. `thinking.type:
  disabled` is refused on the Fable and Mythos models, Opus 5.5 and Sonnet
  5.5, and `between_tools` on every model of Anthropic's per-model thinking
  table but Sonnet 5.5, the 4.x models included. A refusal that depends on
  the effort level is not claimed, and an id the catalog does not know is
  never refused. Each refusal is a fact of the model's capability rule
  (`reasoning_facts.refuses`), read from Anthropic's pages of 2026-11-24.
  Claude Opus 4.8 and 4.7 now have their own rule: a temperature other than
  the default, a `top_p`, a `top_k` or a thinking budget on them is refused
  the same way. These rules, the earlier sampling and thinking-budget ones
  included, apply on Anthropic's own endpoint only: a request sent through an
  operator-pointed base URL (a gateway, a proxy) goes as declared, and that
  endpoint answers for itself.
