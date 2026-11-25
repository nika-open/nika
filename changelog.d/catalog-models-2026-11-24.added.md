- **`nika check` knows the models released since the last catalog.** Claude
  Fable 5.2, Mythos 5.2 and Mythos 5.1 (reachable once Anthropic has
  verified the organization), GPT-6.2 Sol, GPT-6.2 Astra, GPT-6.1 Terra,
  Gemini 4 Neon, Hydrogen, Carbon and Argon, and Kimi K3.1 Pro and K3 get
  catalog rows with limits read from each vendor's page, and admission
  tariffs hashed from the pages they read (Kimi on Moonshot's own endpoint,
  sent `max_completion_tokens`). Capability rules record only what those
  pages state: the Gemini 4 models take audio, image, PDF, text and video
  and always think; GPT-6.2 Sol and Astra and GPT-6.1 Terra take text and
  images with structured outputs; Mistral Large 4 always reasons; Fable 5.2,
  Mythos and the Kimi K3 models carry their always-on thinking and fixed
  sampling. An explicit effort level on the new OpenAI models and the Kimi
  K3 models is still refused before any byte, since the wire would send a
  `thinking` key their APIs do not document. Retirement floors are recorded for
  Gemini 2.5 Flash and Pro, Gemini 3.1 Flash-Lite and GPT-5 mini. The
  defaults move off retiring models: Gemini and Vertex AI to
  `gemini-3.5-flash-lite` (Gemini 2.5 Flash leaves the Gemini API on
  2026-12-01 and left Vertex AI on 2026-10-20), and Moonshot's default and
  cheap model to `kimi-k3.1-pro` (Kimi K2 retires by the end of December
  2026).
