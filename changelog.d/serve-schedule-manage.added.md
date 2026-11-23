- **Schedules can be listed, removed and run by hand over HTTP.**
  `GET /v1/schedules` shows the schedules the door created and the
  project's beats, each with its next slot. `DELETE /v1/schedules/{id}`
  removes one under `If-Match` (the exact `ETag`, or `*`).
  `POST /v1/schedules/{id}/trigger` admits one job now under what the
  schedule declares, with an `Idempotency-Key`; the cadence does not move
  and the next slot still fires. A schedule may also state `owner` and
  `labels`, under the same law as `POST /v1/jobs`: every job it fires or
  a trigger admits carries them, so `GET /v1/jobs?owner=...` finds
  scheduled work too. A schedule that states none keeps its revision.
  Project beats stay declared in `nika.yaml`: removing or triggering one
  over HTTP answers `409 schedule_declared_by_project`. `/health`
  advertises the capability `scheduleManage`.
