- **The Anthropic wire refuses two more requests the current Claude models
  answer with HTTP 400, before any byte is sent.** A forced tool choice
  (`any` or a named tool) on Claude Fable 5.2, Fable 5.1, Opus 5.5 and
  Sonnet 5.5 is refused with the repair (`auto`, or a structured output),
  and so is a provider extra `thinking.type` the model does not take:
  `disabled` on the always-thinking models and on Sonnet 5.5, whose lower
  setting is `between_tools`, and `between_tools` everywhere else. Claude
  Opus 4.8 and 4.7 now have their own capability rule: a temperature other
  than the default, a `top_p`, a `top_k` or a thinking budget on them is
  refused the same way, as Anthropic's thinking page of 2026-11-24 states.
