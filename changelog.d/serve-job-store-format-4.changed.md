- **The resident's job store moves to format 4.** Every job now records
  when it was admitted and its rank in the store, and can carry the owner
  and labels its requester states. A format-3 store is rewritten as
  format 4 by the resident that takes the state root, once it holds the
  server lease, with every event hash, receipt and identity digest
  unchanged; a start that loses the lease to a live resident writes
  nothing. A migrated job gains no invented admission date. An engine
  older than this one refuses a store this one has written, so keep a copy
  of the state root while a downgrade must stay possible. `/health`
  reports `storeFormatVersion.jobs: 4`.
