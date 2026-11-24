- **`nika wire detected` sees every host `nika wire` can connect.** Its
  detection read a six-row list of its own beside twenty-one wire targets,
  so a machine with Codex, Gemini CLI, Copilot CLI, Amp or Kimi Code
  answered "no MCP client detected" and a Codex config was never brought up
  to date. Each wire target now says where its config lives, the writer and
  the detection read that one answer, and a client added to `wire` is
  detected with no second list to remember. `nika doctor` and `nika welcome`
  report those hosts too once their config is on the machine: the six they
  always listed keep their rows, and a host that is absent gets no new line
  and no config. `detected` means the file the writer would patch is already
  there: OpenCode and Junie are seen from the project file `wire` writes,
  Continue once it has been wired, and a repository that ships a
  `.cursor/mcp.json` no longer makes `detected` create a Cursor config in a
  home that has none. `detected` now wires host by host: a config its writer
  refuses is reported on its own line with the reason and left as it is, the
  other hosts are wired, and the exit code is 3 so a script sees the skip. A
  client wired by name and `wire all` still write nothing when one config is
  refused.
