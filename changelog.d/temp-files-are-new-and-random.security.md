- **Nine writers that publish a whole file by rename now stage it in a
  new file of their own.** Each staged the file under a temporary name
  that could be known in advance and opened it in a way that follows a
  link, so an entry already at that name could receive the bytes:
  `nika check --fix` and the registry cache since 0.100.0, the MCP pin
  lockfile and the anchor sidecar since 0.106.0, the OpenTelemetry
  export since 0.119.0, the values kept beside a journal and the saved
  session-intelligence choice since 0.123.0, and `nika:write` and the
  blob store since 0.90.0. Each temporary file is now created only as a
  new file, never through a link, with its mode set when it is made,
  then written through that file and renamed into place. The values
  kept beside a journal are still synced before the rename, as before;
  the others are not, as before. Every writer keeps the mode it gave
  before: `nika check --fix`, the pin lockfile, the registry cache, the
  export and the anchor keep the mode of the file they replace, the kept
  values stay owner-only, and the rest are created as a plain new file
  would be. The writers that already refused an existing name (since
  0.113.0) can no longer be stopped by one placed there ahead of time. A
  model download still resumes from its fixed `.part` name in the models
  folder of the account.
