- **`url` is past a stack-overflow advisory.** In `url` 2.5.8, reading
  the origin of a `blob:` URL descended into the URL it wraps without a
  bound, and a URL nested deeply enough overflowed the stack
  (RUSTSEC-2026-0592). The engine reads the origin of `http` and `https`
  URLs only: the crawl of `nika:fetch` compares links its extractor has
  already narrowed to those two schemes, and the other readers take a
  vetted or operator-set endpoint. No path was found by which a fetched
  page or a workflow reaches the overflow; the audit reads every
  lockfile, though, and the advisory refused each push. The three
  lockfiles (the root, `fuzz/` and `crates/nika-acp/`) move to `url`
  2.5.9 with the `form_urlencoded` 1.2.3 it requires, and nothing else
  in them changes. 2.5.9 also brings the parser closer to the URL
  Standard: a `^` in a path is percent-encoded, so a fetched URL that
  carries one now sends `%5E`; a `file:` URL keeps its host before a
  drive letter; a `\` after the port of a non-special scheme is refused.
