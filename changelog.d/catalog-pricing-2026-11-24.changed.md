- **The embedded model pricing snapshot is refreshed from models.dev as of
  2026-11-24.** 660 priced rows replace 727: 87 added (among them Claude
  Fable 5.2, GPT-6.2 Sol and Astra, GPT-6.1 Terra, Gemini 4 Neon, Hydrogen
  and Carbon, and their OpenRouter routes), 154 retired upstream removed,
  and the statuses models.dev marks deprecated recorded. `mistral-large-latest`
  now prices Mistral Large 4. A trace that pinned the 2026-10-12 table
  (`sha256_16 abd9babb167a88e4`) now refuses its cost replay on this engine,
  as an unknown table version never silently re-prices.
