- **Tests no longer start a real host program.** A test that reached a
  presence check, a sign-in check, a version probe or a session through the
  shipped harness rows started the program the row names (`codex`,
  `codex-acp`, `claude`, `gemini`, `copilot` and the others) when it was
  installed on the developer's machine, under their own sign-in, and the
  `nika welcome` tests wrote the developer's
  `~/.nika/inference-choice.json`. The repository's `.cargo/config.toml`
  now sets `NIKA_HARNESS_HOSTS=off` for every process Cargo starts
  (`cargo test`, `cargo nextest run`, `cargo run`): each host program is
  then absent, not found on `PATH`, its sign-in unreadable, its probe or
  session refused as a missing binary (NIKA-1803), and nothing starts.
  `nika doctor` and `nika welcome` then say each runtime was not looked
  for and name the switch, never that it is not installed.
  `NIKA_HARNESS_HOSTS=on` starts hosts again for a live suite or a
  `cargo run`; an installed `nika` reads no Cargo configuration and starts
  hosts as before. The welcome tests keep their seat choice in a home of
  their own.
