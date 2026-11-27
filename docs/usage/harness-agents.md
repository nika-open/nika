# What `permits:` covers on a harness seat

An `agent:` task can run on an agent application you are signed into, a
**harness seat** (`run: { access: { via: codex, protocol: acp } }`,
`nika run --access codex`, or a seat the access plan discovers). The harness
then runs its **own** tools: its file edits, its shell, its fetches. Nika does
not execute them, so it cannot judge each one the way it judges an `exec:` task
or a `nika:*` builtin.

Nika holds the session with two levers, and `nika check` prints the result on
every such task as a `[harness]` hint (`nika explain harness`).

## What Nika enforces

**Before the prompt** (journaled as `permit_checked`, plane `agent`):

- The session works in the project the run was admitted for, never in the
  directory the Nika process happens to run from.
- Nika selects the harness's measured **asking** mode and reads it back from
  the harness's own answer (Gemini CLI only acknowledges the switch), or
  refuses to start. It never selects a mode that approves actions on its own
  (an auto-reviewer, accept-edits, full access, bypass, yolo). A mode change
  the harness reports during the turn ends the session (Gemini CLI reports one
  only as message text, which Nika does not read).
- Nika offers the harness no file or terminal methods. A harness that calls one
  anyway gets an error, never a fake success, and the refusal is journaled.
- Nika asks the harness to report a turn it could not complete as a failure
  (the `sessionFailure` extension, which codex-acp and claude-agent-acp
  implement). A rejected model or an exhausted retry limit then fails the task
  with the harness's own error, instead of returning that error as the answer.
  A failure the harness offers to retry is transient (`NIKA-1804`, so `retry:`
  may replay the task); one that asks you to sign in again reads as an expired
  sign-in; any other is a refusal (`NIKA-1805`). A harness without such a
  report still returns its error as text, and Nika does not read answer text
  to guess a failure.

**On every permission the harness asks** (journaled as `permit_checked`):

- Inside your `permits:` grants: allowed once, never "allow always".
- Outside them: the run pauses with the harness's question, followed by the
  command it names unless the title shows it, and by the reason it gives
  (`NIKA-1806`). The question is one line: control characters become spaces
  and invisible ones (direction overrides, zero-width characters) are removed,
  so the harness cannot rewrite or hide the line you answer. A title or
  command over 2,000 bytes shows its beginning and its end around the number
  of bytes left out and their SHA-256. Answer with
  `nika run --resume <trace> --answer <task>=true` (or `false`). The harness
  is shut down at the pause; the resumed run starts the task again in a new
  session, and your answer decides its first ask outside the grants, once for
  the task: an attempt that `retry:` replays never reuses it, so an ask outside
  the grants there pauses again.
- An answer of `false` refuses the request: Nika selects the harness's own
  one-time refusal ("reject once"), the first one it offers, never "reject
  always". What happens next is the harness's: some refusal options let the
  agent go on without the action, others end the turn, which then fails
  `NIKA-INFER-001`. In the codex-acp 1.13.1 source, as read, a command's
  default options put "No, continue without running it" before "No, and tell
  Codex what to do differently", which by its label interrupts the turn, and a
  file change or a command that asks for more permissions offers only the second. A harness
  that offers no one-time refusal is answered `cancelled`, the protocol's word
  for calling the turn off, so the turn may end there too.
- Paths are judged in your project's terms (`./out/**` matches the absolute path
  the harness names) and where the operating system will land: each path is
  walked from the root, component by component, through every symlink (a
  dangling one included) and every `..`. `out/link/../notes.md` with `out/link`
  pointing outside the project is judged outside, beside the link's target. A
  path that cannot be resolved (a symlink loop, an unreadable directory) pauses
  for you.
- A command is matched against `permits.exec` by its program. A harness that
  asks with a shell string (Codex, Claude Code) or without any command line
  (Gemini CLI) can only be admitted by `permits.exec: true`; otherwise the ask
  pauses for you.
- In `read-only`, Codex runs ordinary commands in its sandbox without asking,
  so a command it asks about asks to leave that sandbox, and Codex would run it
  with no sandbox at all. Such an ask always pauses for you, `permits.exec:
  true` included, with the command and Codex's reason in the question.
- An ask for network access is never answered automatically, whatever
  `permits.exec` grants: a fetch, or a command the harness says needs the
  network (Codex: an http or https URL, a SOCKS target such as
  `socks5Tcp network access to <host>`, or a network permission). It pauses for
  you with the target in view.
- A command that asks the harness to widen its own sandbox (Codex
  `additionalPermissions`, Claude Code `dangerouslyDisableSandbox`) pauses for
  you: no grant covers it, `permits.exec: true` included.
- An MCP tool call the harness asks you to approve always pauses for you,
  because `permits.tools` names your workflow's MCP tools, not the harness's.

## What `permits:` does not cover

What the selected mode lets the harness do **without asking** never reaches
Nika, so `permits:` does not bound it:

| Seat (measured on) | Its default | Mode Nika holds | Still done without asking |
|---|---|---|---|
| `codex` (codex-acp 1.13.1, codex 0.156.1) | `agent`: Codex's own reviewer approves | `read-only`: every approval goes to Nika | reads any file; writes the project, `/tmp`, `$TMPDIR` and its own `~/.codex` (generated images, session logs, state); runs any program inside its own sandbox, and runs its tools through a JavaScript "code mode"; calls its configured MCP tools marked `readOnlyHint: true`, remote ones included, so a Codex MCP approval rarely reaches Nika; opens pages with its hosted web tool, even under `permits: {}`, so untrusted web content reaches the model; starts every configured MCP server when the session begins; applies your global `~/.codex/AGENTS.md` and your Codex plugins. This adapter has no read-only sandbox: **even with no `fs.write` grant, the project stays writable.** Network stays off for its commands unless it asks. |
| `claude-code` (claude-agent-acp 0.81.1, Claude Code 2.1.280) | your `permissions.defaultMode` (`auto` lets a classifier decide) | `default` ("Manual"): edits and commands ask | reads files in the project; runs commands Claude Code classifies as read-only; fetches documentation hosts it pre-approves; applies a managed Claude Code policy when the machine has one. `bypassPermissions` is withheld, and your own Claude configuration is not loaded (see below). |
| `gemini-cli` (0.62.0) | `default`, or your `--approval-mode` / settings | `default` | reads files in the project (its file tools refuse paths outside it); runs shell commands it judges safe reads; Google web searches. |
| `qwen-code`, `kimi-code`, `opencode`, `copilot`, `grok-build` | — | none: no mode is measured, so none is selected; a current mode that visibly approves on its own refuses the run | everything it does without asking. Nika judges only what it asks. |

Each measured row (`codex`, `claude-code` and `gemini-cli`) holds for the
adapter that names itself as the row's adapter in its `initialize` answer,
on the release the row names. The `codex` row holds for an adapter that
answers `@agentclientprotocol/codex-acp` or `codex-acp` (the older
`@zed-industries/codex-acp` package answers `codex-acp`), the `claude-code`
row for `@agentclientprotocol/claude-agent-acp` and the `gemini-cli` row for
`gemini-cli`. On another release of that same adapter that the seat still
accepts (the `codex` seat takes 0.16 through every 1.x release, the
`claude-code` seat 0.23 through every 0.x release, the `gemini-cli` seat
0.37 through every 0.x release), and on a `gemini-cli` session whose adapter
reports no release, Nika applies the same row: it selects and reads back the
same mode, refuses to start when the adapter does not offer or keep it, and
still reads a Codex command ask as one that leaves its sandbox. What it says
changes: after the row's text, the `permit_checked` record adds one
sentence, for example "What the harness does without asking was measured on
claude-agent-acp 0.81.1; this session's adapter reports release `0.112.0`,
on which it was not measured." When the adapter does not offer the mode or
does not confirm it, the refusal before the prompt ends with the same
sentence. What the harness does without asking may differ on that release.
`nika check` reads neither the installed adapter's name nor its release, so
its `[harness]` hint states the seat's row as it is, without that sentence,
whichever adapter is installed.

An adapter the seat admits under another name has no measured row and is
treated like the seats of the table's last line: no mode is selected, a
current mode that visibly approves on its own refuses the run, and Nika
judges only what it asks. On the `claude-code` seat, the deprecated
`@zed-industries/claude-agent-acp` package answers under its own name, so
its sessions run this way.

A release outside the accepted range is refused before any session. The
`codex` and `claude-code` seats judge the release the adapter reports in its
`initialize` answer, and their refusal names the accepted range and what to
install: the maintained package at the release the row was measured on, such
as `@agentclientprotocol/codex-acp 1.13.1`; an answer that reports no
release is refused there too. The `gemini-cli` seat judges what
`gemini --version` prints: outside its range, the refusal names the range
and says that a version Nika has not read speaks a dialect it cannot judge,
with no release to install.

On the `codex` and `claude-code` seats, when the adapter names itself as one
of the two rows' adapters, a model named for a run or for a Session
conversation is selected only through the session's model option and read
back, on every release of that adapter the seat accepts (on the `codex`
seat, the older `codex-acp` package included). A session that does not list
the model there refuses before the prompt, even when its older model list
names it: claude-agent-acp has no request to select from that list, and
codex-acp's sets the effort together with the model, in a request whose
answer Nika cannot read back. An adapter the seat admits under another name
keeps the older model list.

On every seat:

- An action Nika allows runs under the harness's own sandbox, not Nika's exec
  sandbox, so `permits.fs` and `permits.net` do not confine it. Nika judges the
  network access a harness asks for, not the connections an allowed command
  opens by itself: Codex keeps its sandboxed commands offline unless they ask,
  while a command Claude Code or Gemini CLI runs has the network the machine
  has.
- On every seat but `claude-code`, the harness's own configuration still
  applies: its permission rules, its hooks and the MCP servers it configures
  act without Nika's judgment unless they ask.

A `claude-code` session starts without your Claude configuration. Nika opens
it with the user, project and local settings files turned off, with no MCP
server, and with auto memory and claude.ai connectors off. So the session
loads none of these:

- what `~/.claude/settings.json`, `.claude/settings.json` and
  `.claude/settings.local.json` carry: permission rules (allow, ask and deny
  alike), sandbox settings, hooks and `env` entries;
- `CLAUDE.md` files, the project's auto memory, and the skills, agents and
  commands kept in a `.claude/` folder;
- your plugins, with their skills, agents and MCP servers;
- the MCP servers of `.mcp.json` and of your user settings, and your
  claude.ai connectors.

The agent keeps its built-in tools, judged by `permits:` through the asks
described above, and works from the task's `system:` and prompt. A task that
needs an instruction file says it in its prompt; a task that needs an MCP tool
calls it from the workflow (`invoke:` with `tool: "mcp:<server>/<tool>"`).

A rule that narrowed what Claude Code may do goes with the rest. A `deny` or
`ask` rule, or a sandbox setting, written in one of those three files no
longer applies to the session: a file such a rule kept Claude Code from
reading is read like any other file of the project, without an ask. A
restriction that must hold on this seat belongs in a managed policy, which
still applies.

Three things stay outside Nika's reach on this seat:

- A managed policy, installed on the machine or set by your organization,
  still applies, with whatever rules, hooks and servers it carries, and so
  do the plugins built into Claude Code.
- The global `~/.claude.json` is still read, and Claude Code still saves the
  session's transcript in its own configuration folder under your home
  (both move with `CLAUDE_CONFIG_DIR` when you set it).
- The adapter reads the settings files itself for four values: the mode a
  session starts in (Nika replaces it with `default`), the model of a task
  that names none, the models it offers and its default effort.

A task that names no model (or names `default`) runs on the one your Claude
settings name, or on Claude Code's default when they name none. Nika selects
that model itself before the prompt and reads it back, so the trace records
the model the session started on. A session that rejects that selection, or
does not keep the model until the prompt, ends before any prompt is sent. A
model the session moves to afterwards, during the turn, is recorded and
never refused: only a model the task names refuses the answer, under
`fallback: none`. Name a model on the task to choose it in the workflow.

A sign-in, a cloud provider switch, a proxy or a certificate bundle that
exists only in one of those settings files (an `apiKeyHelper`, or `env`
entries) is among what the session no longer loads. Sign in to Claude Code
itself, which keeps that sign-in outside those files. For a proxy or a
certificate bundle, set `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY` or
`NODE_EXTRA_CA_CERTS` in the environment Nika runs in: Nika hands those four
to the Claude Code seat (the three proxy names in lower case too), beside
`CLAUDE_CONFIG_DIR` and the few names every seat's process receives (`HOME`,
`PATH` and the like). It hands over no name shaped like a credential and no
cloud provider switch, so the seat runs on Claude Code's own sign-in. A
proxy address that carries a user and a password is handed over as you
wrote it.

A `codex` session does not open when it would see a project's Codex
configuration. codex-acp marks the directory of every session it opens as a
trusted project, and Codex then applies the `.codex` entry of that directory
and of each parent up to the repository root (the first directory whose
`.git` resolves; outside a repository, the directory alone): its
`config.toml`, with the MCP servers, permissions and hooks it may define, and
the rest of that folder. Nika looks for such an entry right before the
session's process would start, on every kind of session of the seat: an
`agent:` task, an ACP `infer:` turn, an authoring session, an offer listing
and a conversation. When it finds one, the session fails as a refusal that
`retry:` does not replay, and the message names each entry it found. An
`agent:` task reports it as `NIKA-1805` when the run declares its access; an
ACP `infer:` turn reports `NIKA-1805` always. A `.codex` that is a link
counts as present, wherever it leads and even when it leads nowhere. Move the
entry out of the project, or run the task from a directory that has none.

- The seat is recognised in three forms: the `codex` row, a command whose
  file is named `codex-acp`, or a row that declares one of the names
  codex-acp reports at `initialize`.
- Your own Codex home (`CODEX_HOME`, or `~/.codex` when it is unset) is your
  user configuration, which applies to every session anyway: a session
  rooted in the directory that holds it is not refused for it, also when
  that directory is reached through a link, or when your home is itself a
  link into a checkout (a session in that checkout opens). A project's
  `.codex` that is a link to your home still counts. A home that is not
  an absolute path (a relative `CODEX_HOME` or `HOME`) exempts nothing.
- A scratch directory that Nika creates for a one-shot session or a
  conversation is judged like any directory; with the default temporary
  directory, no repository root sits above it.
- Nika finds the repository root by `.git`. A `.git` that does not resolve
  (a link that leads nowhere) does not end the walk: it goes on to the first
  `.git` that resolves or, when none does, to the outermost directory that
  holds one. A `project_root_markers` setting in your Codex configuration is
  not read.
- An entry that cannot be looked at (in a directory Nika may not search,
  for instance) counts too, named with the reason. A session directory
  named with `..` is refused, since the directories Codex would read cannot
  be judged from it (a temporary directory takes its name from `TMPDIR`);
  `.` parts are dropped.

The `initialize` probe that reads an adapter's name and release is not a
session: it starts in Nika's own directory and ends before any session
exists, so it is not looked at.

codex-acp also makes model calls of its own that are not in the run's
token counts or cost and are not recorded in the trace:

- **The session title.** codex-acp 1.13.1, the release this seat is measured
  on, names each new session itself. After the session's first completed
  turn, it sends the text of that first prompt to a second model of the same
  Codex account (`gpt-5.6-luna` in that release), in a separate temporary
  thread. That thread starts without the seat's profile, on your own Codex
  configuration, in the session's directory. This release offers a client
  no way to set a title, so Nika cannot prevent that call. The seat admits
  codex-acp 0.16 to 1.x for `agent:` tasks and exactly 1.13.1 for ACP
  `infer:` turns, authoring and conversations; the other releases in that
  range were not read for this.
- A release that lets a client set the title (`sessionCapabilities.title` in
  its `initialize` answer; codex-acp 2.12.0 does, and is outside what the
  seat admits today) receives `session/set_title` from Nika on the `codex`
  seat right after `session/new`, before any selection and before the first
  prompt. Every kind of session is titled but an offer listing, which gets
  no title. The title names the kind of session (`Nika agent`, `Nika infer`,
  `Nika authoring` or `Nika conversation`): never the prompt, the system
  text or a value. Nika waits for the answer up to the `initialize`
  deadline (30 seconds by default), or half the session's frame bound when
  that is shorter, and the session goes on whatever comes: an answer that
  arrives after the wait, a result or an error, is passed over. A
  conversation's opening record says what the adapter did (`title_call`:
  `accepted`; `refused` for an error answer; `unanswered` when no answer
  came in that time), and the other session kinds record nothing. By
  2.12.0's documentation, an accepted title stops the adapter's own title
  writes. The same documentation says Codex may refuse to title a session
  that has no first message yet: the record then says `refused`, and the
  adapter may still name the session from its first prompt. Neither was
  observed on a live seat. No other seat receives a title call, and neither
  does an adapter that does not advertise the capability.
- **Prompt suggestions.** Every session on the `codex` seat starts its
  adapter with `CODEX_ACP_PROMPT_SUGGESTIONS=false`. codex-acp 2.12.0 reads
  it to turn off the follow-up prompts it would otherwise generate with a
  further model call for a client that declares JetBrains' AIR extension,
  as Nika's `agent:`, `infer:` and authoring sessions do to receive typed
  turn failures. 1.13.1 does not read it and makes no such call. No other seat
  receives it.

If a task must not write anything, or must not read outside the project, do
not rely on `permits:` alone on a harness seat: run it on an API or local model
(the native agent loop judges every tool call), or in a container or a
throwaway checkout.

## Limits a task declares on a seat

A seat runs its own agent loop, so Nika sends a limit the task declares only
where the seat takes it as a session option. Nika never runs the task as if
the limit were not written: it sends the limit, or the run refuses the task
before anything is sent (`NIKA-465`, the field named), and `nika check` names
the same task as a blocker.

| Field | `claude-code` | Every other seat |
|---|---|---|
| `max_turns` | sent as the session's `maxTurns` | refused |
| `temperature` | refused | refused |
| `tools:` | refused | refused |

- On the `claude-code` seat, Nika sends `max_turns` with the session's other
  options when the session opens, as `maxTurns` (an option a client may set,
  as read in the source of claude-agent-acp 0.81.1). Claude Code counts the
  turns, not Nika.
- Measured on a live seat with claude-agent-acp 0.81.1 on 27 November 2026,
  in a workflow that declares `run.access`: limits of 1 and 2 stopped a task
  that needed three model requests, and a limit of 8 let it complete. The
  session reported each stopped turn as failed, in its own words
  (`Reached maximum number of turns (N)`, N being the limit sent), and the
  task failed `NIKA-1805`. Not measured: a seat reached without a declared
  `run.access`, whether the count is per prompt or per session, any other
  release, and the deprecated package the seat still admits.
- A first turn that stops at the limit gets no schema repair prompt, and
  fails:
  - as a turn the application reports as failed: `NIKA-1805` when the
    workflow declares `run.access` (the case measured), `NIKA-INFER-001`
    when it does not (read in the code, not measured);
  - with the stop reason `max_turn_requests`, a shape Nika handles but did
    not observe on a live seat: `NIKA-INFER-001`, whether or not the
    workflow declares `run.access`.

  A repair, when one is sent, is a second prompt of the same session under
  the same options; how Claude Code counts it against the limit is its own.
- The native agent loop fails an exhausted `max_turns` with
  `NIKA-AGENT-001`; a seat never does. On a seat a stop at the limit is
  `NIKA-1805` or `NIKA-INFER-001`, as above, because the application reports
  its turn limit and its budget limit in the same shape, and gives its retry
  limit the same stop reason: the stop does not say which limit was reached.
  A `retry.on_codes` or `on_error.on_codes` list written for `NIKA-AGENT-001`
  does not match a seat's stop. Those lists can name `NIKA-INFER-001` but not
  `NIKA-1805`, which is outside the form they accept: under a declared
  `run.access`, catch the stop with an `on_error:` that has no `on_codes`.
- A task without `max_turns` sends no limit, and the session keeps Claude
  Code's own loop. The default of 10 turns is the native agent loop's, never
  a seat's.
- The limit follows the seat's id, not the name the adapter reports. On the
  `claude-code` seat it is sent to whichever adapter that seat started,
  including an older one the seat still admits (the deprecated
  `@zed-industries/claude-agent-acp`), whose source Nika has not read. An
  adapter you declare under another id is refused `max_turns`, even when it
  answers as Claude Code.
- Nika sends no seat a sampling temperature: drop `temperature:` or run the
  task on an API or local model.
- `max_tokens_total` is judged from the counts the application reports (see
  below); a seat that reports none leaves it unjudged
  (`token_ceiling: unjudged`).

## Images and structured answers on a seat

An application such as Codex can generate images during an `agent:` turn.
Declare `images:` on the task to use them in the workflow:

```yaml
banner:
  agent:
    prompt: "Draw a 16:9 release banner, no text"
    images:
      save: ./out/release/banner.png   # optional
halftone:
  with:
    banner: ${{ tasks.banner.output.images[0].path }}
  invoke:
    tool: nika:image_fx
    args: { input: "${{ with.banner }}", out: ./out/release/banner-halftone.png, ops: [{ halftone: {} }] }
```

- `.output` becomes `{ message, images }`. `message` holds what `.output`
  would hold without `images:`. Each image carries `index` (from 0), `path`,
  `sha256`, `size_bytes`, `mime_type`, `width` and `height`.
- Only the bytes the application sent on **this task's own session** count.
  The application also reports a path where it kept a copy (Codex: a folder
  under `~/.codex/generated_images` that every session on the machine
  shares). Nika never opens that path and never lists it as an image, so a
  task cannot pick up another session's image. Do not ask the application to
  copy its image into the project; declare `save:` instead.
- With `save:`, Nika writes the received bytes at that path through
  `nika:write`, inside `permits.fs.write` (`NIKA-SEC-004` outside it). A
  second image lands at `banner-2.png`, a third at `banner-3.png`, up to
  four. Every path is judged before any file is written: `nika check` judges
  all four for a literal path (grant the directory, `./out/release/**`, or
  the four files `--infer-permits` lists), and the run judges the received
  ones before the first write. The written file has the digest of the
  received bytes. A task whose session delivered no image fails
  `NIKA-AGENT-007` instead of succeeding with no file.
- A save is a write: two tasks that can run at the same time and save on one
  path, a `for_each` task that saves one path, or a save beside a
  `nika:write` of one of its files are refused (`NIKA-SEC-012`), like two
  `nika:write` tasks. Order them with `after:` or give each its own path.
- Nika recognises the image tool of Codex. On another application,
  `images: { save: }` receives nothing and fails `NIKA-AGENT-007`.
- `images:` belongs to a task on a seat. On an API or local model, `nika
  check` names it as a blocker and the run refuses it (`NIKA-465`): the
  engine's own loop gets its images from `nika:image_generate`.

A turn ends the task only when the application says it finished
(`end_turn`). A turn cut at the model's output limit (`max_tokens`) fails
`NIKA-AGENT-006`. A refusal (`refusal`), the limit of model requests for the
turn (`max_turn_requests`: the task's `max_turns` when it was sent to the
`claude-code` seat, otherwise the application's own), a cancelled turn
(`cancelled`)
and a stop word Nika does not know fail `NIKA-INFER-001` with the reason
named. A turn ends `cancelled` when the application called it off, on its own
or after a refusal whose option ends the turn, or when it stopped on a request
Nika could only cancel because the application offered no way to refuse it
and go on; the task does not complete. Either way the text received is kept
at `partial_output`, masked like every failed record, and the failure records
no token counts, even when the application reported some. An ACP answer must
name its stop reason: one that names none is malformed and fails. A seat
backend that reports no stop reasons at all completes the task as before.

The task records what the application reported about the turn. The token
counts of its answer to the prompt ride as `tokens`, `tokens_in` and
`tokens_out`, with `usage_source: adapter_prompt_result`; an answer that
carries none (Codex sends `usage: null` on a turn that failed) leaves
`tokens` unknown and says `usage_source: unreported`, never 0. The context occupancy of its latest
`usage_update` rides as `context_used` and `context_size`. Claude's session
cost rides as `cost_estimate`: the application's own estimate, cumulative for
the session, with its currency and `billed: false`. Nika keeps a cost only
when its currency is three capital letters (`USD`) and its amount a finite
number of zero or more; any other cost is dropped, and the record shows none.
It is never `cost_usd`, no cost ceiling reads it, and an update that omits it
clears it. Counts too large to add up, in one turn or
across a turn and its repair, are recorded as unknown, never as a capped
number: the record then says `usage_source: unreported`, although the
application sent them. The model Claude reports having answered with is the
task's `model_reported` (source `adapter_usage_update`) and the selection
receipt's `responder`, beside the model the session confirmed. Under
`fallback: none`, only a reported model that is demonstrably another one
(another model line, version or release date) than the model the task named
refuses the answer (`NIKA-1805`), as a model the application moved during
the turn does; a spelling Nika cannot place is only recorded, and a task
that named no model is never refused for a model that moved during the
turn. The counts are the application's word: Nika does not verify them, and
a subscription seat stays unpriced.

A `schema:` on such a task is validated by Nika: the prompt carries the
schema, the final message must contain a JSON value that satisfies it, and
the validated object becomes the output (`message` under `images:`). The
final message is the turn's last one. A tool call always starts a new
message, and so does a chunk whose `messageId` differs from the current
message's, or a chunk with an id after chunks without one; a chunk without
an id stays in the current message. Narration before a tool call is
therefore not parsed as the answer. When the turn held several messages,
the trace keeps its whole text beside the value (`seat_transcript`, its
first 64 KiB, cut after the run's secrets in it are masked); a turn of one
message keeps no `seat_transcript`. One turn may hold 4 MiB of text. A final
message that does not validate gets one repair prompt in the same session,
which still holds the agent's work: the prompt lists every violation and the
schema and asks for the final message only, and the trace shows it as an
`agent_nudge` with reason `schema_repair`. The repair is one more prompt on
your subscription and must fit the task's `max_tokens_total`: counts the
first turn reported at or over the ceiling refuse it (`NIKA-AGENT-002`), and
an application that reported no counts leaves the ceiling unjudged, which
the trace says (`token_ceiling: unjudged`). A repaired
message that still does not validate, a session the application closed
with its final message, or a repair turn that never answers (the
application exits, answers with an error or goes silent), fails
`NIKA-INFER-002`; the failure's `details` keep
the rejected message (`partial_output`) and its violations, each worded
without the value it judged or a key the message holds (an unexpected key
is counted against the names the schema allows, a path segment the schema
does not declare reads `*`) and at most 256 bytes long. An `on_error:`
recovery reads the whole message, its secrets masked as in every failed
task's record;
the trace keeps its first 64 KiB, cut after the run's secrets in it are
masked. A
new session never replays the work: `on_codes: [NIKA-INFER-002]` in a
`retry:` block is the way to ask for a fresh attempt.

An `infer:` task over ACP (`run.access.protocol: acp`) runs one tool-free
turn. It carries:

- `schema:`: the prompt carries the schema and Nika validates the answer. An
  answer that does not validate is never the output: Nika asks again in a
  new one-turn session that receives the earlier answer and the errors, up to
  the repair budget, then fails `NIKA-INFER-002`. Each repair is one more
  prompt on your subscription.
- `vision:` files, sent as ACP image blocks, when the application declares
  image prompts at `initialize` (codex-acp and claude-agent-acp do). An
  application that does not is refused before the session starts, and so is
  a file whose bytes do not start with a PNG, JPEG, GIF or WebP signature.
  As on an API route, a `vision:` file is a read inside `permits.fs.read`:
  `nika check` judges a literal path, and the run judges every path on its
  resolved form before the session starts (`NIKA-SEC-004` outside the
  boundary, `NIKA-AUTH-006` at check with no `permits:` block). A granted
  file that cannot be read fails with the read's own code
  (`NIKA-BUILTIN-READ-001` when no file is there, `-002` otherwise), and a
  secret in a vision entry is the `infer` leak (`NIKA-SEC-006`) unless its
  `egress:` sanctions `{ to: "infer" }`.

It cannot carry `temperature`, `max_tokens`, an enabled `thinking` budget or
a `vision:` URL (Nika does not fetch it). The run refuses those before
anything is sent (`NIKA-432`), and `nika check` names them as blockers instead
of reporting the file run-ready. On a seated task, `check` also stops
suggesting `max_tokens` or `max_tokens_total`.

## Asking a seat for its fast tier

A workflow can ask its seat how fast to serve, beside the model and the
effort:

```yaml
run:
  access:
    via: codex
    protocol: acp
    fallback: none
    service_tier: fast   # or: standard
  reasoning:
    effort: low
```

- `service_tier` takes two words, `standard` and `fast`. They are the
  language's words. Codex's own values (`on`, `off`, `priority`) are refused
  in the file.
- Leaving the key out is not `standard`: Nika then sends no speed selection,
  the session keeps its own tier, and the receipt records none.
- On the codex seat, Nika selects the model, the effort, the tier and the
  session mode in that order, and reads the whole selection back before the
  first prompt. `fast` is sent as codex-acp's `fast-mode: on`, `standard` as
  `off`.
- The session decides whether the selected model offers a fast tier. Codex
  lists the option only for a model that supports one. When the option is
  missing, or the read-back does not hold the value, the task fails
  `NIKA-1805` before any prompt is sent. Nika never serves `fast` at the
  regular tier.
- The binding was measured on codex-acp 1.13.1 and is held for that release.
  Another release of the adapter refuses a declared tier until it is
  measured: install 1.13.1, or remove the key.
- Only an ACP session carries a tier. An API or local route, a seat with no
  measured speed control (claude-code today) and an `infer:` task outside
  `protocol: acp` are refused by `nika check` and by the run before task 1
  (`NIKA-1800`).
- A child workflow reads its own `run.access`. It does not inherit the
  parent's tier.
- A resumed run whose tier changed re-runs its `infer:` and `agent:` tasks.

What the receipt can and cannot say. Each such task's `access_selection`
gains a `service_tier` block:

```json
{
  "requested": "fast",
  "option": "fast-mode",
  "offered": ["off", "on"],
  "transmitted": "on",
  "configured": "on",
  "configured_source": "confirmed_selection",
  "turn": { "value": null, "evidence": "unknown" },
  "effective": { "value": null, "evidence": "unknown" }
}
```

`configured` is what the session reported after the selection. `turn` (the
tier carried on the turn) and `effective` (the tier that served it) read
`unknown`: no ACP answer names either. A configured `on` is not proof that a
given turn ran on the fast tier. Two things outside Nika's sight decide it:

- Codex applies a tier only while its own `fast_mode` feature is enabled
  (it is by default). Check it on the seat with `codex features list`.
- A fast tier uses more of your subscription's allowance per turn.

On a short answer the tier changes little: most of the time is session start
and time to first token. It shows on long outputs.

## Asking a seat what it offers

A run learns what its session offers on the way to its prompt, and says so
only when it refuses. `nika serve` can ask ahead, without a prompt, for a
workflow it serves:

```sh
curl -s http://127.0.0.1:8787/v1/access-offers/workflows/brief.nika \
  -H "Authorization: Bearer $(cat .nika/serve.token)"
```

`/health` lists `accessSessionOffer` when the server has this route. The
answer is a `nika/access-offer@1` document:

```json
{
  "schema": "nika/access-offer@1",
  "workflow": "workflows/brief.nika",
  "status": "read",
  "declared": { "via": "codex", "protocol": "acp", "fallback": "none", "effort": "low" },
  "prompts_sent": 0,
  "routes": [
    {
      "via": "codex", "protocol": "acp", "profile": "completion", "tasks": ["brief"],
      "status": "observed",
      "observed_at": "2026-11-10T17:00:00Z", "fresh_until": "2026-11-10T17:01:00Z",
      "adapter": { "name": "@agentclientprotocol/codex-acp", "version": "1.13.1" },
      "model": { "requested": "openai/gpt-5.5", "status": "confirmed", "current": "gpt-5.5",
                 "offered": ["gpt-5.4", "gpt-5.5"], "omitted": 0 },
      "effort": { "status": "offered", "option": "reasoning_effort",
                  "offered": ["low", "medium", "high"], "omitted": 0, "current": "medium" },
      "service_tier": { "status": "offered", "option": "fast-mode",
                        "offered": ["standard", "fast"], "omitted": 0, "current": "standard" }
    }
  ],
  "routes_omitted": 0
}
```

- The route is the file's. Nika asks only when the workflow declares
  `run.access.via` under `run.access.protocol: acp`. Any other workflow
  answers `"status": "not_applicable"` with the reason, and no session is
  opened. A request names a served workflow and nothing else.
- There is one row per session shape and model: `completion` for the models
  `infer:` tasks resolve to (the route's one-shot completion profile),
  `agent` for the models `agent:` tasks resolve to.
- Each session is opened as a run opens it, selects the file's model for
  that session only, lists what the session has for that model and ends. No
  effort, tier or mode is set and no prompt is sent, so an offer uses no
  turn of your subscription.
- `effort.offered` holds the exact values a run accepts on
  `run.reasoning.effort` for that model: no case folding and no alias.
  `effort.current` is what a run that declares no effort keeps on a session
  like this one. `service_tier.offered` holds the language's words
  (`standard`, `fast`), read through the adapter's measured speed option.
- A model the session does not list is `"status": "not_offered"`, with the
  models it does list in `offered`. Its efforts and tiers are then
  `unobserved`: they are never read from another model's configuration. The
  same holds for `legacy_list` (the session names the model only in its
  legacy model list) and `not_confirmed` (it answered another model). When
  the adapter names itself as the codex or claude-code row's adapter, a
  legacy list is never read: a model only it names is `not_offered`, as a
  run refuses it.
- The tiers are `unobserved`, with the reason, on an adapter or a release
  whose speed control is not measured (anything but codex-acp 1.13.1 today).
- A session that gives no offer is a row with `"status": "unavailable"`, the
  code a run would meet before its prompt (`NIKA-1803` adapter absent,
  silent or outside its pin; `NIKA-1804` session death; `NIKA-1805`
  refused) and a reason.
- A row is an observation with a time. It is kept 60 seconds (15 when
  unavailable) and served again meanwhile, so a page that asks often opens a
  session rarely. One reading runs at a time; a ninth concurrent request
  answers `503 access_offer_busy`.

An offer authorizes nothing. A model, an effort or a tier can leave a seat
between the offer and the run: the run selects, reads back and refuses on
its own session exactly as before.

## Reading the trace

Each harness `agent:` task journals, in order:

- one `permit_checked` with plane `agent`, gate `session mode · <mode>` and
  decision `confine` (the measured mode was selected) or `unmeasured` (the
  harness's own mode was only observed),
  whose `why` states what the harness still does without asking (on a
  release its row was not measured on, followed by the sentence naming the
  release measured and the one the adapter reports);
- one `permit_checked` per ask, with decision `allow`, `deny` (your answer) or
  `escalate` (paused for you). Its gate names what was judged: `mcp · <server
  or tool call>` for an MCP tool approval, `network · <target>` for a network
  ask, `sandbox · <widening>` for a sandbox widening (its `why` names the
  command the widening is for), `sandbox · command` for a Codex command that
  asks to leave its sandbox (the command and the reason in its `why`, whole up
  to 16 KiB, then its beginning and end around the bytes left out and their
  SHA-256),
  otherwise the ask's kind with its program or paths (an unresolvable path
  reads `<unresolved>`). A run paused on that question still journals the
  task's decisions, just before its `workflow_paused` frame;
- one `permit_checked` with decision `deny` for each file or terminal method the
  harness called and Nika refused;
- one `agent_image_observed` per image: the digest and size of the received
  bytes, the local blob, the path the harness reported (never opened) and,
  under `images: { save: }`, the `saved_path` Nika wrote. `nika serve` lists
  that file among the run's artifacts with the received digest.
