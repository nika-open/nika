- **The run guard's warning when `nika` is missing shows once even
  without a state folder.** Since 0.125.0 the hook remembered that it had
  warned with a marker under `${XDG_STATE_HOME:-~/.local/state}/nika/`,
  so with no `HOME`, or a state folder it could not write, it warned on
  every shell command. It now keeps the marker as a folder of the user's
  own under the temporary folder in that case, and removes it once
  `nika` is found again.
