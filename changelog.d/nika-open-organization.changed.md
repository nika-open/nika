- **Nika moved to the `nika-open` organization.** The engine and its
  sibling repositories left `supernovae-st` with their history, releases
  and issues, and every old address redirects. From this release the
  archives, `SHA256SUMS` and their attestations come from `nika-open/nika`
  (verify with `--repo nika-open/nika`), the image is
  `ghcr.io/nika-open/nika`, Homebrew installs `nika-open/tap/nika`, and the
  npm packages are `@nika-open/nika` and `@nika-open/nika-check-wasm`.
  Releases up to 0.124.4 keep the names they were published under; their
  attestations verify with `--owner supernovae-st --signer-repo
  supernovae-st/nika`. `nika welcome`, the registry client, the agent kit
  that `nika init` embeds and every `repository` field point at
  `nika-open`.
