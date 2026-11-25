- **The catalog router asks more often in the grown catalog.** The seventeen
  recipes moved every score, so the router's confidence floor rose from 5.5
  to 12.5, derived again from the measured scores: vague phrases and a French
  request keep asking which job is meant instead of routing on one shared
  word. « translate my docs folder to spanish » now routes to
  localization-factory instead of asking. More words in `-es` and `-ies`
  now match their singular: « summarizes » reads as « summarize » (it read
  « summariz »), « cases » as « case » and « summaries » as « summary ».
  Some short requests that routed in 0.125.1 now ask which template you
  mean, naming the best match first, such as « is there a template for
  invoices? », where two invoice jobs fit.
