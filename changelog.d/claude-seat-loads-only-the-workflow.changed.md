- **An `agent:` task on the Claude Code seat no longer loads your Claude
  configuration.** Nika opens the session with the user, project and local
  settings files turned off, no MCP server, and auto memory and claude.ai
  connectors off. Their permission rules (allow, ask and deny alike),
  sandbox settings, hooks, `CLAUDE.md` and memory files, skills, plugins
  and MCP servers are not loaded; the agent keeps its built-in tools,
  judged by `permits:` through its permission asks. A `deny` or `ask` rule
  that narrowed Claude Code in one of those files no longer applies: a
  restriction that must hold belongs in a managed policy, which still
  applies, as do the plugins built into Claude Code and the global
  `~/.claude.json`. A task that relied on an instruction file says it in
  its prompt, and one that used an MCP server of yours through the seat
  calls it from the workflow (`invoke:` with
  `tool: "mcp:<server>/<tool>"`). A task that names no model still runs on
  the one your Claude settings name; Nika now selects it itself and reads
  it back, so the trace records the model the session ran on. A sign-in, a
  cloud provider switch, a proxy or a certificate bundle set only in a
  settings file (`apiKeyHelper`, `env`) is no longer loaded: sign in to
  Claude Code itself, and set `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY` or
  `NODE_EXTRA_CA_CERTS` in the environment Nika runs in, which Nika now
  hands to this seat. `nika check`, `nika explain harness` and the run's
  `permit_checked` record say what this seat loads.
