- **A kept run keeps every child trace it started, and `nika trace verify`
  names a child trace that is missing.** A `for_each` over
  `invoke: { workflow: ./round.nika }` wrote one child trace per item, but
  the item rows did not name them, and trace retention counted each child
  as a run of its own workflow: with 41 items, the next run kept the ten
  newest children and removed the others, while the parent that ran them
  stayed. Each item row now names the trace its child workflow wrote
  (`child`, the trace file name), whatever the row's status: a `recovered`
  row names it beside the child's error code, and a `cancelled` row, an
  item the batch stopped while its child ran, names the trace that child
  had opened. A task that `on_error:` skipped or recovered keeps its
  `child` row too; before, a failed child an `on_error: skip` passed over
  was cited nowhere and went at the next collection. Retention now counts
  a run by its root trace: a child never counts toward its workflow's kept
  count, stays as long as its parent stays, even past the age cap, and
  goes with its parent, for the parent's reason. A paused child never
  goes. The size budget removes a whole run, the parent with its children.
  A child whose parent is already gone is an ordinary trace. A run still
  in flight cites its children only once its fan-out ends, so while it
  runs no trace written after it began is rotated or removed for the size
  budget: a `nika run` started beside a long fan-out no longer removes the
  children that fan-out has not cited yet. `nika trace
  verify` on a parent now adds one line for each child trace it cites that
  is not beside its journal, naming the trace, its task and, for a
  `for_each`, the item; the tier and the exit code do not change.
