- **The run guard's hook refuses a judge that has not answered within 20
  seconds, and an answer that is not a whole verdict.** Since 0.107.0 a
  judge that never answered left the hook waiting until the host timed it
  out, and a host that times a hook out lets the command through; the
  hook also passed on whatever the judge printed, and a signal that ended
  it left a status hosts read as a non-blocking error. The hook now ends
  a judge still silent after 20 seconds, with every process it started,
  and refuses the command as `guard_unavailable`. It passes on only one
  line that ends, holding one JSON object, and on the judge's exit 2 only
  a deny: an answer cut short, a second line, a banner or anything else
  is refused the same way, on success included. Any other end of the
  hook, a signal or an unexpected failure included, is the blocking exit
  2, and ends the judge with it. The Claude Code and Cursor wiring of the
  plugin and of `nika init` give the hook a 30-second timeout, above that
  deadline. A very large workflow can take longer than 20 seconds to
  judge, and is then refused.
