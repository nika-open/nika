- **`nika serve` can forget the jobs it settled long ago.** Out of the box
  the job store keeps everything and refuses every admission once it holds
  10,000 jobs. `--job-retention <DURATION>` (`36h`, `7d`) forgets a
  succeeded job once it has been settled that long, and
  `--job-retention-unsuccessful <DURATION>` sets the window of failed,
  cancelled and interrupted jobs, which otherwise follow the first. Jobs
  leave when the next one is admitted and at startup; a queued, running or
  paused job never leaves. A forgotten job's `Idempotency-Key` is
  forgotten with it, so keep the window longer than your callers retry.
  Trace journals are not removed. The minimum is one minute, and both
  flags work without `--bind`.
