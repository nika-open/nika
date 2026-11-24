- **An MCP server no longer starts unconfined on a host with no OS
  sandbox.** Where neither Seatbelt nor bubblewrap is present, a server
  declared in `.nika/mcp_servers.json` used to start unconfined behind an
  `UNSANDBOXED` note, while an `exec:` child on the same host was refused
  with `NIKA-1710`. The spawn now rides the same `NIKA_SANDBOX` switch:
  under `auto` (the default) and `require` it is refused with `NIKA-1710`
  and no process starts, in `nika mcp approve` (an environment exit) and in
  `nika run`, where the call fails with it and is not retried (an `invoke:`
  task fails, an agent reads it as that tool's error). A server always runs
  inside a boundary (the project tree and its `network` arm), so `auto`
  requires confinement for it as it does for an `exec:` child under
  `permits:`. `NIKA_SANDBOX=off` is the explicit waiver: the server starts
  unconfined, its note names the waiver, and every `nika run` attests it on
  the journal's opening frame (`sandbox_policy: off`, `sandbox_waived:
  true`), whatever its workflow names. A mistyped `NIKA_SANDBOX` now refuses
  every spawn with `NIKA-1711`, on a host with a backend too. A sandbox that
  is present but refuses the profile stays `NIKA-MCP-005`.
