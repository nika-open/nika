- **Every MCP shelf entry now carries its own verification date and a
  status.** `scripts/refresh-mcp-servers.sh` probes each package on npm,
  PyPI or its OCI registry and sends each hosted remote an MCP `initialize`,
  then dates only the entries that answered, records the version each
  registry answered in its `--json` report, and records the counts in the
  shelf's `[meta]` block: 113 of 120 verified on 2026-11-24, none gone. A run
  that cannot reach half of its registries exits 3 and writes nothing, and
  registry text is stripped of control characters before it is printed.
  Each entry's `status` is `current`, `deprecated` (the 7 whose npm package
  is deprecated upstream, which keep their older date) or `restricted`
  (Ahrefs, whose terms permit its hosted endpoint only from the AI clients it
  lists, not from custom or standalone MCP clients), with a `status_note`
  saying why; `nika catalog --mcp` prints it on the row and `--mcp --json`
  carries both fields. The build refuses a remote that is not `https://` and
  a control character in a title, description, note, package or URL, and the
  listing prints any it meets as U+FFFD. The oracle's `nika_catalog` returns
  the shelf as compact JSON and takes a `query` that keeps the servers whose
  id, alias, title, category or tag contains it. Intercom's entry names its
  Streamable HTTP endpoint (`/mcp`) instead of the deprecated `/sse`, and
  entries that named a community package for a vendor that publishes its own
  server now name the vendor's.
