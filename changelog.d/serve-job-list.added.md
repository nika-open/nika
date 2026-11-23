- **`GET /v1/jobs` lists jobs, newest first.** An application finds its
  jobs again by `owner`, `status` (several at once), `workflow`, `origin`,
  `label=key:value` (repeatable, all must match), `created_after` and
  `created_before`, with `limit` (1 to 200, default 50) and an opaque
  `cursor`. A page is a list of light summaries: no inputs, no outputs, no
  receipt, and the same redacted error as the job itself. Pages are
  stable: a job admitted while a caller pages never repeats or hides
  another. An unknown query member or a value outside its bounds is
  refused with `400 invalid_job_query`, never ignored. Filtering by
  `owner` is not an authorization boundary: whoever holds the token lists
  every job. `/health` advertises the capability `jobList`.
