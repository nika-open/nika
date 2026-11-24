- **A scheduled slot whose workflow changed after it fired no longer
  stops `nika serve`.** When a slot that already had its job was offered
  again under an edited workflow (a deploy between a crash and its
  restart, or a schedule removed and declared again while the slot was
  still on time), the resident ended on an idempotency conflict, and
  ended again at every restart inside the slot's window. The slot now
  stays consumed against the job it fired: nothing runs twice and the
  resident keeps serving.
