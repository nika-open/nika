- **A cost cap no longer refuses an unpriced model on a harness seat.**
  A run seated on a harness (Codex, Claude Code and their kin) that named
  a model the catalog does not price was refused under any
  `--max-cost-usd`, with NIKA-1709 ("cloud model … is unpriced"), right
  after `nika run` had printed that the seat's subscription bounds the
  seat's own spend. Two gates judged the cap one after the other: the
  command's preflight read the access plan and admitted the run, then the
  run's own launch gate judged it as unseated and refused it. Measured
  with nika 0.125.1 on the Claude Code seat and `anthropic/haiku`:
  refused as unpriced under a cap of half a dollar, while the same file
  with no model named was admitted. Both gates now ask one function
  (`nika_runtime::seated_on_harness`), and it answers as the router does:
  a run is seated when the plan names a seat for every model the launch
  can resolve. So a pinned seat also seats a model that only a binding
  names. A run with a model no seat serves, one on a key for instance,
  is not seated and keeps the whole cap. On a seat the cap still refuses
  an id this binary cannot resolve, an unpriced paid media call and a
  priced floor above it.
