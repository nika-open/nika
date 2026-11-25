- **`nika check` reads an `agent:`'s `images.save:` as part of its effect, as the run does.** An
  input only the save path reads is now listed in the requirements (`inputs_read`), and a
  `lift: taint` whose binding only the save path reads is no longer refused as a door that lifts
  nothing (`NIKA-AUTH-011`).
