- **A Codex session that would see a project's `.codex` configuration no
  longer opens.** codex-acp marks the directory of every session as a
  trusted project, so Codex applied the `.codex` entry of that directory
  and of each parent up to the repository root, with the MCP servers,
  permissions and hooks it may define, outside Nika's judgment. Nika now
  looks for such an entry right before the process of any session on the
  `codex` seat starts: an `agent:` task, an ACP `infer:` turn, authoring,
  an offer listing and a conversation, a scratch directory Nika creates
  included. It refuses the session and names each entry found
  (`NIKA-1805` for an `agent:` task under a declared access, and for an
  ACP `infer:` turn always). A `.codex` that is a link counts, wherever it
  leads, and so does an entry that cannot be looked at; a session
  directory named with `..` is refused. The seat is the `codex` row, a
  command named `codex-acp`, or a row that declares a name codex-acp
  reports; your own Codex home, named as an absolute path, is not
  counted. Move the entry out of the project, or run the task from a
  directory that has none.
