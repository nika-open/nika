- **The secret journey `nika check` prints names only what the secret flow
  found.** The verdict was right; the JOURNEY and SECRETS rungs described
  the file wrongly in two places.
  - **A secret that is never sent.** Since 0.107.0, the journey attached a
    secret to every task naming it in its request, `with:` or `for_each:`,
    not to the tasks the secret flow found it reaching. A secret a task
    binds or iterates over but never sends was listed as flowing to the
    task's destination, raised the class to `regulated` and was printed as
    a sanctioned flow, while the flow proved it never left. A secret's
    tasks and destinations now come from the flow the verdict reads, so
    the two cannot disagree. Such a secret stays a used secret, since the
    run resolves it, and goes nowhere.
  - **A rule that pins another host.** Since 0.117.0, a sanctioned flow to
    a host said `pins no host` whenever the secret's `egress:` rules did not
    pin that host, even when they pinned another one. The row now names the
    hosts the rules pin (`pins crm.example.com, not other.example.com`),
    says when a rule beside them pins none, and names each sink once. It
    keeps `pins no host` for rules that pin none.
