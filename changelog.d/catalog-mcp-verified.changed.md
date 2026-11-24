- **Every MCP shelf entry now carries its own verification date.**
  `scripts/refresh-mcp-servers.sh` probes each package on npm, PyPI or its
  OCI registry and sends each hosted remote an MCP `initialize`, then dates
  only the entries that answered and records the counts in the shelf's
  `[meta]` block: 113 of 120 verified on 2026-11-24, 7 whose npm package is
  deprecated upstream keep their older date, none gone. Entries that named a
  community package for a vendor that publishes its own server now name the
  vendor's.
