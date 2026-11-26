- **`nika guard` reads only a regular workflow file of at most 4 MiB.**
  Since 0.107.0 the guard read the file a `nika run` names, whatever it
  was, so a named pipe in its place kept it waiting, and a host that times
  a hook out then let the command run unjudged. The guard now opens that
  file without waiting on it and refuses anything else, or anything
  larger, as `guard_unavailable`, with the path and the reason.
