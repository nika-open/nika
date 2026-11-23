- **A job can carry the owner and labels its caller states.** The by-name
  form of `POST /v1/jobs` accepts an optional `owner` (1 to 128 bytes of
  `[A-Za-z0-9._:@-]`) and optional `labels` (at most 16 pairs: a key of 1
  to 64 bytes of `[A-Za-z0-9._/-]`, a value of at most 256 bytes), so an
  application can find a user's jobs again. They are attribution inside
  one authority domain, never an authorization boundary: whoever holds the
  token reads every job. Outside their bounds they are refused with
  `422 invalid_job_owner` or `422 invalid_job_labels` before a job exists.
  They bind the request identity, so the same `Idempotency-Key` under
  another owner is `409 idempotency_conflict`. A snapshot body carries
  none, and a cost review binds none. `/health` advertises the capability
  `jobAttribution`: require it before sending either member.
