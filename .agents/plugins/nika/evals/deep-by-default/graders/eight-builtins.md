---
type: llm
---

Read the workflow file the agent wrote and count the distinct `nika:` builtins it invokes (for example `nika:glob`, `nika:read`, `nika:jq`, `nika:validate`, `nika:assert`, `nika:json_diff`, `nika:write`, `nika:inspect`, `nika:date`, `nika:hash`).

PASS if it uses at least eight distinct builtins, or uses fewer and the agent's reply names the phase it left out and why.
FAIL otherwise.
