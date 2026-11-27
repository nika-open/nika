- **Nika titles a Codex session before its first prompt when codex-acp
  lets a client do so, and turns codex-acp's prompt suggestions off.** On
  the `codex` seat, an adapter that advertises `sessionCapabilities.title`
  at `initialize` receives `session/set_title` right after `session/new`
  (an offer listing gets no title), titled with the kind of session
  (`Nika agent`, `Nika infer`, `Nika authoring` or `Nika conversation`),
  never with the prompt or a value. No other seat receives it. The answer
  is waited for up to the `initialize` deadline (30 seconds by default),
  or half the frame bound when that is shorter, and the session goes on
  whatever comes: an answer that arrives later, a result or an error, is
  passed over. A conversation's opening record says what the adapter
  answered (`title_call`: `accepted`, `refused` or `unanswered`). By
  codex-acp 2.12.0's documentation, an accepted title stops the title it
  would otherwise generate from the first prompt with a second model
  call; the same documentation says Codex may refuse to title a session
  that has no first message yet. Neither was observed on a live seat.
  Every session on the `codex` seat starts its adapter with
  `CODEX_ACP_PROMPT_SUGGESTIONS=false`. codex-acp 1.13.1, the release the
  seat is measured on, makes no suggestions but lets no client set a
  title: to title a session it sends the first prompt's text to a second
  model of the same account, outside the run's token counts, and Nika
  cannot prevent it.
