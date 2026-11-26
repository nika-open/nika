- **The run guard hook blocks a deny by its exit code.** Since 0.100.0
  the kit's run guard answered a denied `nika run` with a JSON deny on
  exit 0, which only a host that reads that JSON dialect treats as a
  refusal. The hook, and the copies `nika init` writes, now exit 2 with
  the verdict and a fixed `guard_denied` line on stderr, which Claude
  Code documents as the block's message and Codex as its blocking
  reason. Cursor documents exit 2 as a block but no message from it, so
  under Cursor a deny in Cursor's own dialect is still passed on as the
  answer, with exit 0, and its `agent_message` reaches the agent; nika
  guard answers in that dialect only for a payload without
  `hook_event_name`, and any other deny is blocked by exit 2. The hook
  also catches QUIT, XCPU, XFSZ, VTALRM and PROF from its first step
  after cleaning the PATH, exits 2 when it cannot write its answer, and
  keeps the judge's answer under `/tmp` when `TMPDIR` is a relative path.
