- **The SDK guard reads what the published `@nika-open/nika` 0.125.0 closes.**
  `published_sdk.json` is re-read from that package's `dist`, which still enforces machine
  protocol 2, and every closed set of `@supernovae-st/nika` 0.124.0 is unchanged. One parser is
  new: the access offer of `GET /v1/access-offers/{name}`. It closes the keys of the offer, of
  its declared route, of each route row with its adapter and model, and of each listing, and the
  values of the offer's `schema` and `status`, of a row's `protocol`, `profile`, `status` and
  model `status`, and of a listing's `status`. It reads the declared route's values as text, so
  those stay open. The other runtime changes in that `dist` read a task's access rows on local
  frames only and rename the native packages, so they close nothing. The guard also lists a
  value it had missed: the snapshot acknowledgement's `status`, which the SDK already closed to
  `accepted` in 0.124.0.
