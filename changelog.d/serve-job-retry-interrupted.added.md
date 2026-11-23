- **`nika serve` can start a job again when its run loses its owner.**
  A job that was running when the resident stopped or was killed used
  to stay `interrupted`. With `--retry-interrupted restart` the next
  resident runs it again as a new attempt of the same job: same id,
  same `Idempotency-Key`, a new execution and trace identity, and the
  interrupted attempt kept as evidence with its receipt. The default
  is `off`: no job is started again, and each new job keeps its
  effects class on its record. `--retry-max-attempts` (default 3,
  at most 16) bounds the attempts, the first included, and one cost
  ceiling covers them all: an attempt whose spend cannot be read is
  never counted as zero. With `--retry-interrupted resume` the new
  attempt serves the tasks the lost one completed from its journal and
  runs the rest; the journal is judged as `nika run --resume` judges
  one, its chain required, and a journal that does not pass leaves the
  job `interrupted` with `resume_refused` rather than run it from the
  first task. A workflow with effects outside the engine
  is started again only when its job stated `retry`, the new optional
  member of the by-name form of `POST /v1/jobs` (`on_interrupted`,
  `max_attempts`, `max_cost_usd`; `422 invalid_job_retry`). A job
  that is not started again says why on its `interrupted` event:
  `retry_exhausted`, `effects_need_consent`, `unknown_spend`,
  `budget_exhausted` or `resume_refused`. A cancelled job and a job a
  schedule fired on its beat are never started again. `/health`
  advertises the capability `jobRetry`.
