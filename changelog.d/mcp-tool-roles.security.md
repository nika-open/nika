- **An MCP tool can now arm the private-read and egress legs of the lethal
  trifecta.** A tool reads and sends on its server, outside `permits.fs`
  and `permits.net`, so a workflow that read a private inbox through one
  MCP tool and sent mail through another armed the ingress leg only and
  passed `nika check`. `nika mcp approve <server>` now takes
  `--reads-private <tool>`, `--sends-out <tool>` and `--no-role <tool>`:
  the role is recorded beside the pin in `.nika/mcp_pins.json`, and the
  check counts a call to a declared tool as that leg of `NIKA-SEC-009`
  (an `invoke:`, an `agent:` whitelist, or the tool grants of a child
  workflow). A tool with no declared role keeps today's verdict and is
  named by a new `[mcp-roles]` hint, so the check says what it did not
  judge. A role is kept across a re-approval only while the tool's
  definition is unchanged. A role is the operator's declaration, not a
  proof of what the server does.
