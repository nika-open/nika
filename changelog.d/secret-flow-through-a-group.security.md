- **`nika check` follows a secret through a group fold, a recovered value, a
  `for_each:` item and every secret a task's output carries.** The secret
  flow of `nika check` lost a secret on five paths, so a file that sends a
  secret where its `egress:` rules do not allow could check clean.
  - **What was missed.** A `${{ group.<name> }}` read carried none of its
    members' secrets: `check` misjudged it from 0.109.0, the release that
    introduced `group:`, and the run could send the fold from 0.121.0
    (before that the read failed at run with `NIKA-VAR-001`). Since 0.90.0,
    the first release with the secret flow check, `check` did not read a
    task's `on_error.recover:` value, which the run renders with secrets
    resolved in clear and makes the task's output. Since 0.90.0 too, a
    task's output passed one secret on: an output that held two gave later
    tasks and `outputs:` only the first. A `with:` binding that reads `item`
    carried none of the item's secrets, since 0.90.0 as well, though the run
    renders it once per iteration with the item bound. And a `for_each:`
    written as a literal list passed none of its items' secrets on to the
    task's output, since 0.90.0: from 0.112.0 `check` judged the task's own
    request with them, not what later tasks read from its output.
  - **What is followed now.** A fold carries what each member's output
    carries, through `with:` aliases, `for_each:` items and members that
    fold another group. What a task's `for_each:` items carry, written as an
    expression or as a literal list, reaches its request, its `with:`
    bindings that read `item` and its recovered value, and an `exec:` or
    `invoke:` output passes it on (an `infer:` or `agent:` answer still
    carries no secret of its prompt). A recovered output carries what its
    value reads, wherever the task it reads runs, including two tasks that
    recover from each other. An output keeps one path per secret, the most
    direct one, and every request (`NIKA-SEC-006`), every `outputs:` entry
    (`NIKA-SEC-007`, one finding per secret) and the data journey read all
    of them. A group none of whose members received a secret stays clean, a
    destination the `egress:` rules allow stays allowed, and the recover
    itself is no destination.
