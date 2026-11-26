- **`nika check` reads an `agent:`'s `images.save:` as part of its effect, as the run does.** An
  input only the save path reads is now listed in the requirements (`inputs_read`), and a
  `lift: taint` whose binding only the save path reads is no longer refused as a door that lifts
  nothing (`NIKA-AUTH-011`). The output names each saved path, so at run it now carries the label
  of what the save path reads, as when the prompt reads it: a path built from an input or from
  untrusted content makes the output untrusted, and a later task that hands that path to a program
  is judged against `permits.fs.read` like any untrusted path.
