- **`nika check` and the run refuse a secret in an `exec:` working
  directory or a task's `model:`.** The run renders both fields with secrets resolved, but
  the secret flow of `nika check` read neither, so a file could check clean
  while the run handed a secret's value to a program or a provider.
  - **What was missed.** Since 0.90.0, an `exec:` task's `cwd:` was rendered
    with secrets resolved and given to the spawn, while `check` judged only
    the command, `stdin:` and `env:`. Since 0.109.0, an `infer:` or `agent:`
    task's `model:` was rendered with secrets resolved and sent as the
    provider and model of the call, while `check` only noted it as a model
    the run decides.
  - **What is judged now.** A secret reaching `cwd:` or a task's `model:`
    is `NIKA-SEC-006` whatever its `egress:`, since neither a directory nor
    a model name is ever a secret: the finding gives the path from the
    secret to the field and points to `inputs:` or `const:` for a value the
    file does not name. The program can print its working directory, so its
    output carries the secret as well. Both follow a direct reference, a
    `with:` alias, a task output that carries the secret, a `group:` fold, a
    `for_each` item and an `on_error.recover` value. The workflow `model:` is
    unchanged.
  - **In the journey.** A secret reaching a task's `model:` names that task
    in the new `routes` list of the secret in the `--json` journey, and the
    JOURNEY rung names "the model route of" that task as an external flow,
    so the class is `regulated`: the provider receives the route, and
    `check` cannot prove that a route rendered from a secret stays on this
    machine. The row says that no `egress:` rule clears a route and names
    the remedy, in place of the consent review it asks for a host.
  - **At run.** A run holding a report older than this check refuses, before
    it starts, a task whose `model:` or `cwd:` a secret reaches, with
    `NIKA-SEC-006` and a message naming the task, the field and the path
    from the secret. Nothing is rendered, sent or debited for it: no call,
    no ledger line, no spawn, and the journal records only the refusal. An
    `unwind` cleanup task is refused the same way. The run raises the same
    `NIKA-SEC-006` as `check`: one law, one code, whichever of the two finds
    it, and names the same path. The error a failed task records (the text
    later tasks read as `tasks.<id>.error`) no longer carries a resolved
    secret value that an `egress:` rule let a field render, as the event
    stream already did not, and one secret inside another (`abc` in
    `abcdef`) leaves no fragment of the longer one there, in the event
    stream or in the `outputs:` map.
  - **Limit.** The run's refusal follows the same static flow as `check`.
    An `infer:` or `agent:` output carries no secret taint, so a model that
    echoes a secret it was given into a later task's `model:` is not
    caught.
