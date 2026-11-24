- **A `group.<name>` read now carries the secrets its members' records
  carry.** Since 0.109.0, the release that introduced `group:`, the secret
  flow of `nika check` followed `tasks.<id>` reads and not `group.<name>`
  reads. A task that received a secret and joined a group could have its
  record sent anywhere by a task that folded the group: the same file that
  `${{ tasks.<id>.output }}` made `NIKA-SEC-006` checked clean with
  `${{ group.<name> }}`, and the journey line said no secret reaches an
  external destination. The fold is now judged as the same records read one
  task at a time, through `with:` aliases, `for_each:` items and members
  that fold another group: a secret sent where its `egress:` does not
  allow is `NIKA-SEC-006` naming the secret, the task and the path, and the
  journey lists the destination. A group none of whose members received a
  secret stays clean, and a destination the secret's `egress:` allows stays
  allowed. A task's `on_error.recover:` value now rides its record the same
  way: since 0.90.0, the first release with the secret flow check, a
  recovered value that read a secret, or a record that carried one, left
  the task's output clean, so a later task could send it anywhere. The run
  resolves a secret in `recover:` in clear into the record. A secret sent
  on from a recovered record is now `NIKA-SEC-006` with the path through
  the recover; the recover itself is no destination and gets no finding.
