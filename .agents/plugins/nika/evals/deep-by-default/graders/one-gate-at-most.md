---
type: llm
---

PASS if the workflow has at most one human gate (`nika:prompt`), and the agent does not ask the user to approve each step. Nothing in this job leaves the machine, so no gate at all is the expected result; a cost risk is bounded by a `nika:assert` or the run ceiling, not by a question.
FAIL if the workflow has two or more `nika:prompt` tasks, or puts a gate on cost.
