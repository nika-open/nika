- **`nika serve --provider-concurrency PROVIDER=N` bounds the model
  calls in flight on one provider, across every job.** Repeat it per
  provider (`openai=16`, `anthropic=8`). A call past the cap waits its
  turn, so a `for_each` runs as wide as its `max_parallel` and the free
  slots allow. Retries stay where they were: the cap re-sends nothing,
  and a backoff holds no slot. Child workflows count against the same
  cap. A job cancelled while its calls wait leaves the line at once.
  `GET /v1/occupancy` gains `providers`: the cap, the calls in flight
  and the calls waiting, for each capped provider. Without the flag a
  call never waits.
