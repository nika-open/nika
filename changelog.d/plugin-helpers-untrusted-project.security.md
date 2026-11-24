- **The plugin's helpers never run a binary a project supplies.** The edit
  hook used to judge with a build found inside the repository it was
  working in. The binary now comes from the session only: `NIKA_BIN` when
  set, else inside the engine tree the session's own build folder
  (`CARGO_TARGET_DIR`, absolute and outside the tree), else the first
  `nika` on the PATH; `bin/nika-kit` prints which one and why. Text the
  helpers read from a project or from a run's record (names, paths, a
  gate's question) reaches the agent filtered, bounded and marked as data,
  and a path printed inside a suggested command is shell-quoted.
  `/nika:report` writes under the project's `out/` only and follows no
  link there. The stop gate keeps what it has checked across a second
  start of the same session, and the plain verdict, the list of waiting
  runs and the facts read at skill load give the binary a time limit.
