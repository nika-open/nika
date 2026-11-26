- **`nika init` wires an editor hook only to a script it wrote.** Since
  0.100.0 for Cursor and 0.120.0 for Claude Code, a hook script already in
  a client's `hooks-nika/` folder was kept while the settings file that
  runs it was still written, so the editor ran the repository's script
  after every edit. `init` now writes a hook wiring only when every script
  it runs holds exactly what this `init` writes, and otherwise refuses it
  with one report row and exit 3; its copy of the edit hook no longer runs
  anything from the folder beside it. A project an earlier `init` set up
  keeps the copies it got: remove `.claude/hooks-nika/` and
  `.cursor/hooks-nika/`, then run `nika init` again.
