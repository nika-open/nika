- **`nika check` and the run refuse a secret in an agent's `images.save:` path.** The run
  renders the path with secrets resolved, writes each image the turn delivered there and names the
  path in the task's output, but the secret flow of `nika check` did not read the field.
  - **What was missed.** Since 0.124.0, an `agent:` task's `images: { save: }` was rendered with
    secrets resolved on a harness seat, so a file could check clean while the run wrote a file
    whose name held a secret's value and recorded that name in `images[].path`.
  - **What is judged now.** A secret reaching `images.save:` is `NIKA-SEC-006` whatever its
    `egress:`, a rule for the agent's own sink included, since a path is never a secret. The
    finding gives the path from the secret to the field and points to `inputs:` or `const:` for a
    name the file does not write out. It follows every way a secret travels, as for `model:` and
    `cwd:`, and the task's output carries the secret for the tasks that read it. The journey
    counts the task among the secret's tasks, with no destination: the file is written here.
  - **At run.** A run holding a report older than this check refuses, before it starts, a task
    whose `images.save:` a secret reaches, with `NIKA-SEC-006` and a message naming the task, the
    field and the path from the secret. The seat starts no turn, nothing is written, and the
    journal records only the refusal.
