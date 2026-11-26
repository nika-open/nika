- **The run guard hook blocks a deny by its exit code.** Since 0.100.0
  the kit's run guard answered a denied `nika run` with a JSON deny on
  exit 0, which only a host that reads that JSON dialect treats as a
  refusal. The hook, and the copies `nika init` writes, now exit 2 with
  the verdict and a fixed `guard_denied` line on stderr, which Claude
  Code documents as the block's message
  ([hooks](https://code.claude.com/docs/en/hooks)) and Codex as its
  blocking reason ([hooks](https://learn.chatgpt.com/docs/hooks)), both
  read 2026-11-26. Cursor documents exit 2 as a block, equivalent to a
  `permission: deny` answer, and lists `hook_event_name` among the fields
  every hook receives ([hooks](https://cursor.com/docs/hooks), read
  2026-11-26). `nika guard` answers such a payload in the Claude Code
  shape, so on Cursor today the deny is blocked by exit 2, and Cursor's
  documentation does not say its reason reaches the agent. Only when the
  judge answers with Cursor's own deny, exactly as `nika guard` writes it
  for a payload without `hook_event_name` (one flat object holding
  `permission` once, beside at most one `agent_message` and one
  `user_message`, and no other key), is that answer passed on under
  Cursor, with exit 0. The hook also catches QUIT, XCPU, XFSZ, VTALRM and
  PROF from its first step after cleaning the PATH, exits 2 when it
  cannot write its answer, and keeps the judge's answer under `/tmp` when
  `TMPDIR` is a relative path.
