- **`nika check` judges a secret in an `exec:` working directory and in a
  task's `model:`.** The run renders both fields with secrets resolved, but
  the secret flow of `nika check` read neither, so a file could check clean
  while the run handed a secret's value to a program or a provider.
  - **What was missed.** Since 0.90.0, an `exec:` task's `cwd:` was rendered
    with secrets resolved and given to the spawn, while `check` judged only
    the command, `stdin:` and `env:`. Since 0.109.0, an `infer:` or `agent:`
    task's `model:` was rendered with secrets resolved and sent as the
    provider and model of the call, while `check` only noted it as a model
    the run decides.
  - **What is judged now.** A secret reaching `cwd:` is a secret reaching
    the `exec` effect, as one in the command or `env:` is: `NIKA-SEC-006`
    unless the secret's `egress:` allows `exec`, and the program's output
    then carries it. A secret reaching a task's `model:` is `NIKA-SEC-006`
    whatever its `egress:`, since a model name is never a secret: the
    finding gives the path from the secret to the field and points to
    `inputs:` or `const:` for a model the file does not name. Both follow a
    direct reference, a `with:` alias, a task output that carries the
    secret and a `group:` fold. The workflow `model:` is unchanged.
  - **In the journey.** A secret reaching a task's `model:` goes to "the
    model route of" that task, which the JOURNEY rung and the `--json`
    journey name as an external destination, so the class is `regulated`:
    the provider receives the route, and `check` cannot prove that a route
    rendered from a secret stays on this machine.
  - **At run.** The error a failed task records (the text later tasks read
    as `tasks.<id>.error`) no longer carries a resolved secret value, as the
    event stream already did not. What the run sends is unchanged.
