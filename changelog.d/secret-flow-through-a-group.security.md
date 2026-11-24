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
  allowed.
