- **A Codex ACP task on a busy machine is retried by its `retry:` block.**
  Before a session opens, Nika asks the adapter for its version, its
  `initialize` answer and, for Codex, its MCP and feature listings, and it
  gave each answer 10 seconds. On a heavily loaded machine those listings
  sometimes took longer, and the run failed with NIKA-1803 as if the adapter
  were not installed: a class no retry replays, so
  `retry: { max_attempts: 3 }` ran one attempt (since 0.123.0 for the Codex
  listings, since 0.109.0 for the version probe). Every identity and profile
  probe now waits 30 seconds, the same as the `initialize` handshake of a
  session, and `NIKA_HARNESS_PROBE_SECS` (whole seconds, 1 to 600) sets
  another deadline on a machine that needs one. A probe that does not answer
  in time is NIKA-1804, the transient class, so a plain `retry:` replays it,
  and so is the `--version` attestation of a direct one-shot `infer:`, which
  was a final NIKA-INFER-001; a program that cannot be found or started is
  still NIKA-1803 and final, and `nika explain NIKA-1803` now says so. A run
  proves an adapter once: the first good proof serves every later agent task
  and every later ACP `infer:` turn of the run, repairs included, where each
  of them probed again (an unanswered or failed proof is not kept). An error
  answer to `initialize`, `session/new` or a model, effort, tier or mode
  selection is now read like an error answer to the prompt: Codex answering
  `session/new` with an internal error (`-32603`, "workspace routing
  discovery timed out") is NIKA-1804 and retried, where it was NIKA-1805 and
  final (since 0.109.0). A lost sign-in (`authentication_failed`, or ACP's
  authentication-required code `-32000`) stays NIKA-1803 and final, and
  every other error code (parse error, invalid request, unknown method,
  invalid params, resource not found, or a code ACP does not define) stays
  NIKA-1805 and final. Trace retention no longer rotates the evidence of
  a failure away: the newest failed trace of each workflow is never rotated
  or removed for the size budget, failed traces fill a workflow's kept count
  before successful ones, and the budget removes every successful trace it
  may before a failed one (since 0.99.0, ten successful runs rotated a
  failed trace out). The 30-day age cap still ends a failed trace.
