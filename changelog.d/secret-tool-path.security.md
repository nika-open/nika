- **`nika check` and the run refuse a secret in a builtin's path argument.**
  `nika:read` and `nika:write` `path`, a media generator's `output_dir`
  and `filename_prefix`, each `nika:image_compose` `inputs[k]` and every
  other filesystem slot a builtin declares name the file or the directory
  the call reads or writes. A secret there was a send that an `egress:`
  rule for the tool could clear.
  - **What is judged now.** A secret reaching a path argument is
    `NIKA-SEC-006` whatever its `egress:`, the tool's own rule included,
    since a path is never a secret. The finding gives the path from the
    secret to the field and points to naming the path in the file, or to
    `inputs:` or `const:`. Every other argument stays a send the tool's
    rule decides, and the journey counts the path with the call, as it
    counts the call's other arguments.
  - **At run.** A run holding a report older than this check refuses such
    a task before it renders the call, with `NIKA-SEC-006` and a message
    naming the task, the field and the path from the secret.
