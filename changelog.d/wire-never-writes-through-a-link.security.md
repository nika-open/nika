- **`nika wire` and `nika init --wire` write a project's editor config
  only where the project owns it.** Since 0.107.0 `wire` wrote each
  config through a temporary file beside it whose name could be known in
  advance, opened in a way that follows a link, so an entry already at
  that name received the new config and the mode it set. The temporary
  file is now created only as a new file, never through a link, with its
  mode set through the file it wrote. A project config
  (`.vscode/mcp.json`, `opencode.json`, `.junie/mcp/mcp.json`) that links
  to a file of the project, such as `opencode.json` pointing at
  `config/opencode.json`, is still updated where it points and stays a
  link. One that leads out of the project or into its `.git` folder, or
  that sits under a folder that does, is left as it is, with one refused
  row and exit 3, even when the way comes back into the project: each
  link is read one step at a time, and a step outside the project ends
  it. So is one that leads nowhere, when the client is named (`wire
  detected` does not list that client). A config that changes between
  the check and the write is refused, nothing written, and the write
  goes to the place that was checked, without resolving the name again.
