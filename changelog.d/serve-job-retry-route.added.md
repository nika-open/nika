- **`POST /v1/jobs/{id}/retry` starts a settled job again by request.**
  The latest attempt of a job that ended `interrupted` or `failed`
  becomes evidence, and a new attempt of the same job is queued with
  its own execution and trace identity. The body names the attempt
  (`attempt`, the first being 1), the `mode` (`restart` or `resume`)
  and, optionally, `max_cost_usd` for the re-attempts alone; anything
  else is `422 invalid_job_retry`. The request is the caller's consent
  to run the workflow's effects again. The bound of attempts, the
  resume judgment and the job's one cost ceiling hold as for a run
  that lost its resident, and the place in the owner's lane is taken
  before anything is written. The same request sent again answers
  `200` with the job as it stands and starts nothing. Refusals:
  `409 job_not_retryable`, `retry_exhausted`, `resume_refused`,
  `unknown_spend` and `budget_exhausted`, and the queue refusals of an
  admission.
