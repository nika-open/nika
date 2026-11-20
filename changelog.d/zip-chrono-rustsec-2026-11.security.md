- **Four new `zip` 7.2.0 advisories and `chrono`'s unmaintained notice
  are assessed, not waved through.** `zip` 7.2.0 reaches the engine only
  through `candle-core` 0.10.2, the local inference stack the release
  binaries include. RUSTSEC-2026-0545 and RUSTSEC-2026-0547 sit behind
  the `aes-crypto` and `legacy-zip` features, which no build compiles;
  RUSTSEC-2026-0546 and RUSTSEC-2026-0548 sit in `ZipArchive::extract`
  and `ZipStreamReader`, which candle never calls. `chrono`
  (RUSTSEC-2026-0543) arrives only through `feed-rs`, whose latest
  release still requires it. `deny.toml` and `.cargo/audit.toml` record
  each reason and when to look again; candle 0.11, whose `zip` carries
  the fixes, is a separate change.
