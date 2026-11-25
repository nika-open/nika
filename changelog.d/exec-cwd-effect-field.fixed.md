- **`nika check` reads an `exec:` task's `cwd:` as part of its effect, as the run does.** The run
  renders the working directory, labels it and re-gates a tainted one as a path against
  `permits.fs.read`, but the static laws read only the command, its `stdin:` and its `env:` values.
  Three verdicts disagreed with the run:
  - a `lift: taint` whose binding only the `cwd:` reads was refused as a door that lifts nothing
    (`NIKA-AUTH-011`), though the run's re-gate honors it;
  - an input only the `cwd:` reads was missing from the requirements (`inputs_read`);
  - fetched content that picks an `exec:`'s working directory did not complete the lethal trifecta
    (`NIKA-SEC-009`), as the same content in an argument does. The file was still refused by the
    order law (`NIKA-SEC-015`).
