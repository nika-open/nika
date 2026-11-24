- **`nika explain` launches another program only for the two codes that
  print the seat line.** Every explain, from the CLI and from the MCP
  `nika_explain` tool, first asked each installed agentic CLI for its
  login (`claude auth status`, `codex login status`) and read which
  provider keys the environment holds, then kept the answer for
  `NIKA-INFER-001` and `NIKA-1800` only. The question is now asked for
  those two codes alone, and their text is unchanged; every other code
  answers from the tables embedded in the binary and waits for no other
  program.
