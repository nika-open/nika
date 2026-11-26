- **`nika check` and the run refuse a secret in a media builtin's `args.model`.**
  `nika:tts_generate` and `nika:image_generate` call the model `args.model` names, infer their
  provider from it when `provider:` is left out, and name it in the file they save and in their
  output. A secret there was a send that an `egress:` rule for the tool could clear.
  - **What is judged now.** A secret reaching the `args.model` of either builtin is
    `NIKA-SEC-006` whatever its `egress:`, the tool's own rule included, as for a task's
    `model:`, and the journey names the task in the secret's `routes`. Every other argument stays
    a send the tool's rule decides.
  - **At run.** A run holding a report older than this check refuses such a task before the
    call, with `NIKA-SEC-006` and a message naming the task, the `args.model` field and the path
    from the secret.
