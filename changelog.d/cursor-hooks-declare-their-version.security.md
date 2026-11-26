- **The Cursor hooks files declare their version.** Since 0.100.0 the
  plugin's `hooks/cursor-hooks.json` and the `.cursor/hooks.json` that
  `nika init` writes held no `version`, which Cursor's hooks format
  requires. Both files now declare `version: 1`. Neither sets
  `failClosed`, without which Cursor lets the action proceed when a hook
  fails with any exit but 2: on Cursor, as on Claude Code and Codex, a
  hook that cannot start lets the command through, and the run guard
  refuses by its own exit (hooks documentation of
  [Cursor](https://cursor.com/docs/hooks),
  [Claude Code](https://code.claude.com/docs/en/hooks) and
  [Codex](https://learn.chatgpt.com/docs/hooks), read 2026-11-26). A
  project an earlier `init` set up keeps its own `.cursor/hooks.json`:
  add the key by hand, or remove the file and run `nika init` again.
