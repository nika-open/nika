- **`nika serve` shares its workers between owners and takes its limits
  as flags.** Waiting jobs are served fairly: a free worker goes to the
  owner with the fewest jobs running, in turn among equals, so one
  owner's burst no longer delays the others. `--max-queued-per-owner`
  refuses an owner's next job with `429 owner_queue_full` and
  `Retry-After`; `--max-concurrent-per-owner` makes its other jobs wait.
  `--max-concurrent-jobs`, `--queue-capacity`, `--max-jobs` and
  `--shutdown-grace` replace the compiled-in four workers, queue of 64,
  10,000 jobs and 30-second drain. `GET /v1/occupancy` reads what the
  resident holds against these limits, and a queued job that is
  cancelled frees its place at once. `/health` advertises the
  capability `occupancy`. Without a flag nothing changes for a caller
  that states no owner.
