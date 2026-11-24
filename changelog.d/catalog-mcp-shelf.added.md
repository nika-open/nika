- **`nika catalog --mcp` lists the embedded MCP server shelf.** Each server
  shows its title, category, pricing hint, how it is reached (a package or a
  hosted remote with its auth) and the env var names it reads; `--mcp
  --json` emits the `catalog_version: 1` projection, and the MCP oracle's
  `nika_catalog` tool returns the same list with `mcp: true`. The shelf adds
  the tool hubs treg, Composio Connect, Pipedream Connect and Zapier MCP
  (tag `tool-hub`) and the vendors' own hosted servers (GitHub, GitLab,
  Linear, Slack, Stripe, Atlassian, Semrush, Ahrefs, Apollo, Hunter, Neon,
  Square, Plaid, Google Maps, Brave Search, Bright Data and more); Firecrawl
  gains its hosted endpoints and Alexandria provider discovery.
