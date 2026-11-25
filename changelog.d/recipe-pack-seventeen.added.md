- **Seventeen recipe jobs ship in the embedded pack.** `nika try` lists and
  rehearses ci-failure-triage, comfyui-image-chain, docs-answer-desk,
  feed-digest, inbox-triage, launch-campaign-forge, lead-enrichment,
  listing-link-audit, programmatic-seo-pages, qr-art-forge,
  quote-compare-decide, receipt-ledger, refund-chaser, seo-site-audit,
  source-to-45-locales, subscription-renewal-radar and workflow-watchdog,
  each with the fixtures it reads. Seven of them put a confirm gate with no
  default before their first fetch, post, render or scan: unanswered, the
  rehearsal pauses there with no output written, and on `--answer approve=true`
  the whole job rehearses. launch-campaign-forge runs on a Codex seat over
  ACP and refuses the mock backend with NIKA-1801 rather than substitute it.
