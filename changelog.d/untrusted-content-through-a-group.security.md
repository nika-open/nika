- **Untrusted content keeps its label through a group fold, a late recover
  source, an unwind cleanup task and a `for_each:` item.** Nika labels
  content from outside as untrusted (an `exec:` output, `nika:fetch`, any
  `mcp:*` tool, a browsing agent, and at run the caller's `inputs.*`);
  `nika check` refuses that content reaching an egress task under the lethal
  trifecta (`NIKA-SEC-009`) and the run re-gates it on an exec argv or cwd
  and on `mcp:*` arguments (`NIKA-SEC-004`). Until now the label was lost in
  five places, so untrusted content could reach an egress task or a command
  line unlabelled. A `${{ group.<name> }}` fold read as trusted at check
  since 0.109.0, and at run since 0.121.0. An `on_error.recover:` value whose
  source the check reached later (declared after the task, or in a later
  wave) read as trusted at check since 0.106.0. A cleanup task joined by
  `after: { <task>: unwind }` was never judged by the trifecta since 0.109.0.
  Under `for_each:`, a `with:` binding that reads `${{ item }}` and the item
  of a literal list whose strings read untrusted content both read as
  trusted, at check and at run, since 0.106.0. Now a fold carries the label
  of its members' records, an untrusted member named as the source; the
  check repeats its pass until no label moves and refuses every effect if
  the bound is ever reached with a label still moving; cleanup tasks are
  judged after their producers; a literal list carries what its strings
  read; and a binding that reads `item` or `index` is labelled after the
  item, as the run renders it, while the collection reads only the other
  bindings. A `lift: taint` door still names one `tasks.<id>.output` or
  `inputs.<name>` binding and never opens through a fold. `NIKA-VAR-021` now
  quotes a fold as `group.<name>` (it printed `tasks.group.<name>`), names
  `on_error.recover:` as the surface of a recover read (it printed
  `outputs`), and prints the `outputs:` and `model:` surfaces with their
  colon.
