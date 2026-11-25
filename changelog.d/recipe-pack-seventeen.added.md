- **Seventeen recipe jobs ship in the embedded pack.** `nika try` lists and
  rehearses ci-failure-triage, comfyui-image-chain, docs-answer-desk,
  feed-digest, inbox-triage, launch-campaign-forge, lead-enrichment,
  listing-link-audit, programmatic-seo-pages, qr-art-forge,
  quote-compare-decide, receipt-ledger, refund-chaser, seo-site-audit,
  source-to-45-locales, subscription-renewal-radar and workflow-watchdog,
  each with the fixtures it reads. A job with a confirm gate before its
  effect pauses there unanswered and lets nothing leave on
  `--answer approve=false`; launch-campaign-forge runs on a Codex seat over
  ACP and refuses the mock backend with NIKA-1801 rather than substitute it.
