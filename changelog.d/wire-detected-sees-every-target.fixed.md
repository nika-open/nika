- **`nika wire detected` sees every host `nika wire` can connect.** Its
  detection read a six-row list of its own beside twenty-one wire targets,
  so a machine with Codex, Gemini CLI, Copilot CLI, Amp or Kimi Code
  answered "no MCP client detected" and a Codex config was never brought up
  to date. Each wire target now says where its config lives, the writer and
  the detection read that one answer, and a client added to `wire` is
  detected with no second list to remember. `detected` means the file the
  writer would patch is already there: OpenCode and Junie are seen from the
  project file `wire` writes, Continue once it has been wired, and a
  repository that ships a `.cursor/mcp.json` no longer makes `detected`
  create a Cursor config in a home that has none. `detected` now wires host
  by host: a config its writer refuses is reported on its own line, which
  names the host, the file and the reason, and is left as it is; the other
  hosts are wired, and the exit code is 3 so a script sees the skip. A Zed
  or Amp settings file that cannot be read is such a refusal: it used to
  print "manual" and call the file JSONC, exit 0. One that reads but is not
  plain JSON stays manual, and the line now says it may hold comments or be
  damaged. A client wired by name and `wire all` still write nothing when
  one config is refused. `--dry-run` shows the plan and does not test that a
  file can be written: a settings folder that is read-only is reported by
  the run, on a line that names the host and its settings file. Under
  `detected`, on every host whose file `wire` rewrites, an entry that runs a
  command with exactly the args `wire` writes is left as it is, whatever
  that command's path and the entry's other keys, so an absolute path pinned
  for a desktop application is kept; `nika wire <client>` by name still
  rewrites it. An entry with those args and no command, or with anything but
  strings among its args, is repaired. `nika doctor` reads the fifteen new
  hosts by the same rule; its five JSON rows keep the reading they always
  had. Hermes, the one YAML host, is never rewritten: once its `config.yaml`
  is there, `wire` prints the block to add by hand unless the file already
  holds it. Every settings file is read through one guarded reader: a link
  is followed, what it leads to must be a regular file of at most 4 MiB, and
  anything else in its place (a FIFO, a device, a directory) is refused by
  `wire` with its reason and read as not wired by `nika doctor`. A FIFO used
  to block both, and a link to an endless device to read until memory ran
  out. A settings file that is a link is kept as a link when what it leads
  to stays in the same project (a project config) or under the home folder
  (every other): `wire` writes beside the real file, so a config kept in a
  dotfiles folder is updated in place. A link that leads anywhere else, or a
  linked folder that would take a new config elsewhere, is refused on its
  own line and left as it is, and a link whose target is gone counts as no
  file. `nika doctor` and `nika welcome` report the new hosts once their
  config is on the machine: the six they always listed keep their rows, an
  absent host gets no new line and no config, and doctor's coverage line now
  reads 21 probed with nothing declared-not-probed.
