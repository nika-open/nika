- **`GET /v1/jobs/{id}/attempts` lists every attempt of a job.** It
  answers the policy in force for the job, where it stands about being
  started again (`retry_state`: `none`, `queued`, `exhausted` or
  `refused`, with `retry_reason`), and each attempt oldest first: its
  identity, how it was started again (`mode`, `resumed_from`), its
  receipt, what the decision that closed it counted, and when it
  started and ended. Each summary of `GET /v1/jobs` gains `attempts`
  and `retry_state`. `GET /v1/jobs/{id}/trace/verify?attempt=N`
  verifies one attempt's own journal (`400 invalid_trace_query`,
  `404 attempt_not_found`); without the query it verifies the latest,
  as before.
