- **The embedded model pricing snapshot is refreshed from models.dev as of
  2026-11-24.** 681 priced rows replace 727: 87 added (among them Claude
  Fable 5.2, GPT-6.2 Sol and Astra, GPT-6.1 Terra, the four Gemini 4 models
  and their OpenRouter routes), 133 retired upstream removed, and the
  statuses models.dev marks deprecated recorded. 21 rows models.dev no longer
  lists are kept at their last prices, each under a `# kept from` line,
  because the lookup's substring fallback would otherwise price them as
  another model (`o3-mini` as `o3`, `gpt-4.1-nano` as `gpt-4.1`); the refresh
  keeps such rows from now on. Four rows a vendor page contradicts carry the
  page's figure under a `# corrected from` line that survives the refresh:
  Gemini 4 Carbon and Argon output 65,536 tokens and are priced at the rates
  Google applies from 2026-12-01 ($4 input, $20 output, $0.20 cached per
  million), which over-state their cost until 2026-11-30, and Mistral Small
  carries no cached-input price, since Mistral's pricing page lists none.
  `mistral-large-latest` now prices Mistral Large 4. A trace that pinned the
  2026-10-12 table (`sha256_16 abd9babb167a88e4`) now refuses its cost replay
  on this engine, as an unknown table version never silently re-prices.
