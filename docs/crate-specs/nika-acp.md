# Crate spec — `nika-acp` (Gate 1)

| | |
|---|---|
| Status | **SPEC** (P3.1 · D-2026-08-04-N1) — Gate 1 authored 2026-08-05; implementation is P3.2, behind the `access-harness` cargo feature (default OFF). |
| Layer | **L1 effect** — async · one effect family (drive an external agent harness over ACP) · every OS reach behind the confined-spawn seam. |
| Design | The **harness access class** made real: `agent:` tasks execute on the user's OWN authenticated agent harness (gemini-cli · qwen-code · kimi-code · codex-acp · claude-agent-acp), driven through the official Agent Client Protocol — under nika authority (permits bridge), billed honestly (subscription ≠ free), behind an explicit opt-in. The harness owns auth; nika never holds a credential (A-3). |
| Name | `nika-acp` (the protocol is the crate's whole surface — the `nika-mcp` naming precedent). |
| LOC budget | ≤6k src (est. ~3.5k). ≤1500/file, ≤100/fn. |
| Deps | `agent-client-protocol = "=5.6.1"` (exact pin · see §2), the only normal dependency. dev: `nika-harness` (the client judged here), `nika-kernel`, `nika-types` and the other engine crates the e2e files drive, all by path · tokio `1` with its own feature list (no workspace dependency: the crate is its own workspace) · `futures-core` · `tokio-util` (compat) · `serde_json`. The fake agents live in `tests/` (§5). |
| Publish | `false` — engine-internal effect crate. |

## 1 · Why this crate exists (legitimacy is the design constraint)

Token-lifting is dead (the 01→04/2026 purge class); driving the vendor's own
harness through its official protocol is the one durable lane: the harness owns
its auth store, nika drives the session. ACP is the column — v1 stable, an
official Rust SDK, ~35 native agents, org-maintained adapters (`codex-acp`,
`claude-agent-acp`). MCP sampling was deprecated 2026-07-28 and is rejected.

The adapter process is engine INFRASTRUCTURE (like an HTTP socket), NOT a user
`exec:` — but strictly bounded (§4). No route ever changes a verb's meaning:
`agent:` ↔ harness is the natural pairing; `infer:` on a harness is P4's
evidence-gated question, refused here with a witness.

## 2 · Protocol pin (re-verified 2026-11-26 · primary sources)

- Crate `agent-client-protocol` **5.6.1** (released 2026-11-25 ·
  `agentclientprotocol/rust-sdk`), resolved in this crate's own lockfile
  with `agent-client-protocol-derive` 5.6.1 and `agent-client-protocol-schema`
  1.22.0, which the SDK pins exactly. The judge pins the SDK exactly too
  (`=5.6.1`): it enables a draft feature, and the SDK's 5.x migration notes
  say everything behind an `unstable_*` feature may change in a 5.x minor
  release. SDK version ≠ wire version: the judge
  speaks **stable protocol v1** through the SDK's `v1` module (`Agent.v1()`,
  `Client.v1()`, `agent_client_protocol::v1::schema`). Wire v2 sits behind the
  SDK's `unstable_protocol_v2` feature, which the judge does not enable.
- Verified on 2026-11-26. The SDK's release tag was read on GitHub: its
  changelog, the 5.x migration notes, its manifests and the sources of the
  `v1` module. Then, on the repository's toolchain (Rust 1.91.1):
  - `cargo update --manifest-path crates/nika-acp/Cargo.toml -p
    agent-client-protocol` moved three packages (the SDK and its derive crate
    from 2.0.0, the schema from 1.5.0) and removed 18 others, and `tempfile`
    now resolves `getrandom` 0.4.3 where it resolved 0.3.4 (both versions
    were already in the lock);
  - `NIKA_KEYCHAIN=off cargo test --locked -- --test-threads=1`, run in
    `crates/nika-acp` as CI runs it: the whole suite passes (34 cases that
    day);
  - `cargo clippy --locked --manifest-path crates/nika-acp/Cargo.toml
    --all-targets -- -D warnings`: no warning;
  - `cargo metadata --locked` through this manifest, `fuzz/Cargo.toml` and
    the root manifest: the root and fuzz lockfiles do not change.
- Features: the SDK has no default features, and the judge enables one,
  `unstable_end_turn_token_usage`, because the draft `usage` field of a
  `session/prompt` result exists only behind it and the turn usage cases
  write it. The SDK itself always turns on the schema's
  `unstable_proxy_chains`; no case depends on it. Stop reasons,
  `usage_update`, `session/cancel`, `session/close` and `fs/read_text_file`
  are stable v1 and need no feature. Neither `process` nor `stdio` is on:
  the SDK-facing cases connect over in-process pipes (the four e2e files
  spawn their python3 agents through the engine, not through the SDK).
- Earlier pin: 2.0.0 (2026-07-23, verified 2026-08-05). The move to 5.x
  renamed the entry points (`builder()` became `v1()`), the schema path
  (`schema::v1` became `v1::schema`) and the connection type's path
  (`ConnectionTo` became `v1::ConnectionTo`). The protocol version stays 1
  and the messages this judge exchanges keep their shape: the cases that
  existed before the bump pass on the new pin with only those renames.
- A per-version generated-schema diff gate, which would make a wire-v2
  stabilization fail loud at pin-bump time, is not built. This bump was
  checked by reading the SDK and schema changelogs between the two pins.
- SDK roles: Client · Agent · Proxy · Conductor. Nika implements the
  **Client** role only.
- The engine's client stays hand-written on wire v1 (`nika_harness::wire`):
  the SDK judges it from this workspace and is never linked into the engine.
  The SDK's own test agent is not a second judge: it is not published on
  crates.io and pulls in other crates (rmcp, http and more).

### §2bis · The `preserve_order` wall (found 2026-08-05 · the quarantine)

The official SDK requires `serde_json/preserve_order` — **every version
since 0.15.0** (sparse-index verified; wire-v1 stability only exists after).
Cargo feature unification is workspace-global: the moment the dep entered
the diamond graph, `serde_json::Value` flipped from sorted keys (BTreeMap)
to insertion order (IndexMap) across the WHOLE engine, and five
byte-attested goldens turned red (decision receipts · dispatcher receipts ·
urlencoded forms). Byte-attested surfaces are the product's honesty layer —
they can never depend on a vendor SDK's cosmetic feature choice.

**Structural consequence (in force)** · `crates/nika-acp` is a STANDALONE
workspace (root `exclude` + its own lockfile): the mock agent + SDK-side
conformance live there; the SDK's feature graph never unifies with the
engine's. The engine-side design is the OPERATOR'S GATE, two lanes:

- **Lane A (recommended · sovereign)** · the engine speaks wire v1 through a
  small hand-rolled JSON-RPC client (initialize · session/new · prompt ·
  update · cancel · request_permission — the Client role's narrow waist),
  and THIS harness proves conformance from the outside (the official SDK
  plays the agent against our client over in-process pipes).
  Anti-capture by construction; the official SDK stays the judge, never a
  link-time dependency.
- **Lane B (parallel · upstream)** · PR the SDK to feature-gate
  `preserve_order` (it is cosmetic for JSON-RPC). If accepted, Lane A's
  hand-rolled client can retire at a later major — evidence first.

## 3 · Public API (the seam, not the protocol)

```rust
/// One configured harness adapter — id · binary identity · version pin
/// range · per-adapter kill-switch. Ids are UNIQUE and never equal an
/// AccessClass wire string (`harness` as an id would make `--access
/// harness` ambiguous — refused at registry load · R-5d).
#[non_exhaustive]
pub struct HarnessAdapter { /* id, command, args, version_req, enabled */ }

/// A live ACP session on one adapter — serial by construction (one
/// in-flight prompt; the self-queue rides the session, the wave
/// scheduler stays untouched).
pub struct HarnessSession { /* connection, child: KillOnDrop, caps */ }

/// The kernel seam (P3.4 · lives in nika-kernel-ai, not here):
/// `AgentBackendDyn` — sibling of `ProviderInferDyn`, additive.
/// nika-acp provides the impl; nika-verb-agent consumes the trait.

/// Session lifecycle (the Client trait impl):
///   initialize(v1) → session/new { cwd, mcp_servers: [nika-mcp]? }
///   → session/prompt → session/update stream → session/cancel.
/// Every update maps to existing task events (additive fields only).
```

Errors: `NikaErrorCode` one-voice · new rows in the **NIKA-1803..1849** span
(the access block · 1800-1802 taken): adapter-not-found · version-outside-pin ·
handshake-refused · session-died · update-overflow. Each teaches its fix line.

## 4 · Confinement (the nika-mcp discipline, lifted — with ONE deliberate difference)

Lifted from `nika-mcp/src/client.rs:559-604` (the confined-spawn seam):
- OS confinement via the SAME sandbox `confine()` transform (ADR-095 Layer 6) ·
  a refusal is terminal · the receipt line prints once on live spawn.
- `KillOnDrop` child · piped stdio · bounded line reads (update-overflow
  refuses, never OOMs).
- Identity handshake: binary present + `--version` inside the adapter's pin
  range BEFORE initialize; a colliding binary name also proves its exact
  session subcommand through a bounded public help marker (`kimi acp --help`,
  not legacy `kimi --acp`) · controlled argv · no shell.

**The difference (A-3):** NO `apply_env_scrub` of the harness's own auth
store — the harness MUST read its own credentials; scrubbing its env would
break the whole legitimacy model. The child env is: the runner floor ∪ the
adapter's declared passthrough ∪ nothing of nika's secrets. The Claude Code row
passes its config home and the machine's proxy and certificate bundle
(`HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`, their lower-case forms and
`NODE_EXTRA_CA_CERTS`): a session opened without the settings files (§6, 1bis)
no longer reads them from a settings `env` block. No cloud provider switch
crosses: a seat runs on the harness's own sign-in. Documented at the
spawn site; a negative test proves nika's own `NIKA_*_API_KEY` vars never
cross. Auth readiness reads no credential byte: it checks path metadata, obtains
a zero-read read-only handle so the OS applies UID/ACL policy, and requires the
managed-seat witness `credentials/kimi-code.json` (never another provider or
the distinct `mcp/` subtree). It honors `KIMI_CODE_HOME` and fails closed on
absent, empty, ambiguous, unreadable, swapped, or symlinked stores. This is a
configuration witness, not a token-validity claim: the first ACP session owns
expiry/parse judgment and must fail without any native-provider fallback.
A Command-auth row's login command (its exit code alone) has 30 s to answer:
valid checks were measured at 12 s and 25.8 s (2026-10-09), and the former
10 s read a signed-in seat as signed out. A check that does not answer in time,
or cannot run, is an unknown sign-in (`LoginCheck::Unanswered` · `Unreadable`,
carried as `login` on the probe facts), never signed out; admission still
requires an answer (`configured` stays fail-closed).

## 5 · The mock ACP agent (P3.3 · the load-bearing instrument)

Deterministic fake agents speaking wire v1, all in `tests/` and none a
helper bin: an in-process agent on the official SDK's agent role
(`mock_agent.rs` and the SDK-facing cases) and python3 agents the e2e files
spawn. Between them they script turns · permission requests inside AND
outside grants · usage reporting · cancel. Every e2e
below runs hermetic — zero network, zero real harness in CI. A gate proven
only against a real harness is not CI-provable (instrument law).

## 6 · Authority bridge (P3.6 · confused-deputy-free · NIK-266)

A harness runs its OWN tools: Nika serves it no filesystem and no terminal.
`initialize` advertises no `fs` and no `terminal` capability; its
`clientCapabilities._meta` carries one adapter extension, the Air
`sessionFailure` (`jetbrains.air`, version 1, NIK-275) that codex-acp 1.13.1 and
claude-agent-acp 0.81.1 both implement, so a turn the adapter could not complete
(a rejected model, a retry limit exhausted on 429) is reported as a typed
`sessionFailure` on the `session/prompt` result, and the client fails the call
with the adapter's title. The adapter's actions classify it: `login`
(`auth_required`) is the expired sign-in (`HarnessError::Unavailable`, in Nika's
words), `retry` a transient session failure, any other a refusal; codex-acp offers
`retry` on a 429 retry exhaustion and on a rejected model, whose 400 it reports as
a `service` failure. This holds on `infer:` as on `agent:`. Without the extension
codex-acp returns that error as answer text and `end_turn`, which the client cannot
tell from an answer and never pattern-matches; claude-agent-acp rejects the prompt
with a JSON-RPC error instead. An
agent → client call such as `fs/write_text_file` or `terminal/create` is answered
with a JSON-RPC error (`-32601`) and journaled as a `permit_checked` deny, never
an empty success the agent would read as served. Nika's authority over an
`agent:` session is two levers, both journaled as `permit_checked`.

**1 · Root and mode, settled before the prompt** (`nika_harness::client::confine`).
The session root is the admitted project, on the wire (`session/new.cwd`) and as
the adapter process's own cwd, never the host process's directory. The task's
`permits:` project to a confinement (`read-only` without an `fs.write` grant,
`workspace-write` with one), and the client selects the adapter's MEASURED asking
mode and reads it back from the session's own answer (the legacy
`session/set_mode` door answers no state: its success is the acknowledgement), or
refuses before any prompt. A mode that approves on its own is never selected; a `config_option_update`
or `current_mode_update` away from the held mode ends the session. The table is
`nika_types::access::HarnessBoundary` (one source for the client and `nika check`):

| Seat (measured on) | Adapter default | Mode Nika holds | What the harness still does without asking |
|---|---|---|---|
| codex (codex-acp 1.13.1 · codex 0.156.1) | `agent` (Codex's auto-reviewer approves for the client) | `read-only` (approval `on-request`, reviewed by the client, over a `workspaceWrite` sandbox) | reads any file; writes the project, `/tmp` and `$TMPDIR`; runs any program in that sandbox; network off unless it asks. No mode is a read-only sandbox: without an `fs.write` grant the project stays writable |
| claude-code (claude-agent-acp 0.81.1 · Claude Code 2.1.280) | the user's `permissions.defaultMode` (`auto`: a classifier decides) | `default` ("Manual": edits and non-read-only commands ask) | reads files in the project; runs commands it classifies read-only; fetches documentation hosts it pre-approves. `bypassPermissions` is withheld (`_meta.claudeCode.options.allowDangerouslySkipPermissions: false`); the session loads nothing of the user's or the project's Claude configuration (below); a managed policy still applies |
| gemini-cli (0.62.0) | `default`, or `--approval-mode` / settings | `default` (legacy `session/set_mode`: its success is the only acknowledgement; gemini-cli announces a later mode change only as message text, which Nika does not read) | reads files in the workspace (its file tools refuse paths outside it); runs shell commands it judges safe reads; Google web searches |
| qwen-code · kimi-code · opencode · copilot · grok-build | — | none measured: nothing is selected; a current mode that visibly approves on its own (`auto`, `accept`, `full`, `yolo`, `bypass` …) refuses the session | everything it does without asking |

**1bis · A Claude Code agent session loads only what Nika hands it**
(`nika_harness::authoring::acp::agent_meta`). `session/new` carries, in
`_meta.claudeCode.options`, the surface the one-shot and conversation profiles already send,
without the keys that close an agent's own loop and with two environment switches of its
own. The options follow the registry row (`claude-code`) and, for a row declared under
another id, the adapter that answers `initialize` as claude-agent-acp, the name the boundary
row is found by: the session a `permit_checked` row describes is the one that was opened.

The options:

| Option | Value | What it leaves out |
|---|---|---|
| `settingSources` | `[]` | the user, project and local settings files: their permission rules (allow, ask and deny), sandbox settings, hooks and `env`; `CLAUDE.md`; the skills, agents and commands of `.claude/`; the plugins those settings enable |
| `strictMcpConfig`, `mcpServers` | `true`, `{}` | every MCP server but the ones `session/new` names: `.mcp.json`, user settings, plugins |
| `plugins`, `skills`, `agents` | `[]`, `[]`, `{}` | what a client could hand the SDK itself: none is handed (installed plugins and the skills of `.claude/` are already left out by `settingSources`) |
| `env.CLAUDE_CODE_DISABLE_AUTO_MEMORY` | `"1"` | the project's auto memory, which `settingSources` does not control |
| `env.ENABLE_CLAUDEAI_MCP_SERVERS` | `"false"` | claude.ai connectors |
| `allowDangerouslySkipPermissions` | `false` | the `bypassPermissions` mode |

Never sent to an agent session: `tools` (it keeps its built-in tools, each judged through
its permission ask), `maxTurns` and `persistSession`. claude-agent-acp 0.81.1 spreads the
client's options over its own default (`settingSources: ["user", "project", "local"]`), so
each value replaces it. It then sets `cwd`, `permissionMode` and `canUseTool` itself,
recomputes `allowDangerouslySkipPermissions` (the client's `false` holds), drops `agent`, and
merges five keys where it replaces the others: `env` (the client's entries over its own
environment), `mcpServers` (with the servers of `session/new`), `extraArgs`,
`disallowedTools` and `hooks`. Outside these options: a managed policy, the plugins built
into Claude Code and the global `~/.claude.json` are read regardless, Claude Code saves the
session's transcript, and the adapter's own settings reader still takes four values from the
settings files: `permissions.defaultMode` (replaced by the held `default`), `model`, the
model list and the default effort. The CLI no longer reads `model`, and the adapter does not
send it a model it believes the CLI already read. So when the task names no model (or names
`default`), the client selects the model the session reports through its model option
(`session/set_config_option`) and reads it back before the prompt: the recorded model is the
one the session starts on. A session that does not confirm it is refused before the prompt,
one that rejects the selection fails there with the adapter's error, and a later answer of
the session's setup (effort, service tier, mode) that reports another model is refused
before the prompt too. The task asked for no model, so a model the agent moves during the
turn stays a recorded fact, where a move of a model the task named refuses the answer under
`fallback: none`. The hold rides every session this client opens on the adapter that names
itself claude-agent-acp: an `agent:` task, an `infer:` or authoring one-shot, the compile
seat. A conversation does not record the adapter's name and holds nothing; a model option
that states no current value, and a session that states its model only through the legacy
model list, get no request. The three profiles are built from one function, so their shared
keys cannot drift.

**2 · Every permission ask, judged against `permits:`** (`nika_cap::judge_harness_ask`):

- inside a grant → `allow_once` (`allow_always` NEVER · the GOOSE_MODE=auto
  anti-pattern · A-5) · `permit_checked allow`;
- outside every grant → the EXISTING pause (NIKA-1806 · exit 4 · question
  verbatim · `--resume --answer`): the ask is left unanswered and the session ends,
  its adapter's process group killed; `--resume --answer` runs the task again in a
  new session, where the bound answer decides the first ask outside the grants,
  once for the task: the verb reports it spent (`AgentEvent::GateAnswerSpent`) and
  the runtime never hands it to an attempt `retry:` replays, whose ask pauses ·
  `permit_checked escalate`, journaled before the `workflow_paused` frame although
  the paused task never settles; the human gate is never answered by the engine;
- a bound answer `false` refuses its ask → the first `reject_once` the harness
  offers, in its order (`reject_always` NEVER), else `cancelled`, the protocol's
  word for a client calling the turn off · `permit_checked deny`. What follows is
  the harness's: a `reject_once` may let the agent go on without the action or end
  the turn (codex-acp 1.13.1, as read: a command's default `decline` comes before
  `cancel`, which by its label interrupts the turn, and a file change or a command
  asking for more permissions offers only `cancel`); a turn that ends `cancelled` fails
  `NIKA-INFER-001`. The rule is the conversation path's
  (`nika_harness::conversation::answer`). An ask left unanswered (the pause, a
  session torn down) hears `cancelled` whatever it offers;
- the paths an ask names are anchored on the session root (root-relative inside
  it, the vocabulary `permits.fs` is authored in; absolute outside it), and each
  path's physical identity is judged too (`nika_harness::client::anchor`): the raw
  path is walked from `/`, component by component, the resolved prefix kept
  canonical; a symlink is followed through `read_link`, relative to its parent,
  a dangling one included, at most 40 hops; a missing component starts a missing
  tail; `..` pops that tail, else the canonical prefix. `out/link/../x` with
  `out/link` pointing outside lands beside the link's target and is judged there.
  A walk that cannot complete (a loop, an unreadable component) yields a
  `<unresolved>` location no grant admits: the ask pauses, never judged on the raw
  `..` text. The lexical fold only names the displayed location;
- a command is judged by its argv program against `permits.exec`; a shell-string
  command (codex-acp, claude-agent-acp) or an ask with no `rawInput` (gemini-cli)
  is admitted only by `exec: true` (the shell-form law's bridge twin);
- on a seat whose held mode runs ordinary commands in its own sandbox unasked
  (`HarnessBoundary::asked_command_leaves_sandbox`: codex-acp `read-only`), a command
  ask with no other escalation asks to LEAVE that sandbox (`require_escalated`):
  it is an escalation with `escalated_command` set to the command
  (`rawInput.command`, a string or an argv) and the reason
  (`_meta.permission.description`), judged on plane `exec` with gate
  `sandbox · command`, never covered by a grant. Beside a sandbox widening, an
  `execute` ask's `escalated_command` names the command the widening is for, so
  its witness records what ran. The witness keeps the command whole up to 16 KiB,
  then its head and tail around the bytes left out and their SHA-256;
- the pause question is the toolCall title, then the command the ask names unless
  the title shows it, then the request's reason, on one line: every run of
  controls and whitespace becomes one space and invisible characters (direction
  marks, overrides and isolates, zero-width characters, tags) are dropped, so a
  title such as claude-agent-acp's, which is the model's own command, cannot
  rewrite the operator's terminal. A title or command over 2,000 bytes shows its
  head and tail around the bytes left out and their SHA-256;
- an ask that reaches beyond its kind arrives as `HarnessEvent::EscalationAsked`
  and is judged before its kind, never answered within the grants (`exec: true`
  included): NETWORK access, read from every structured signal codex-acp 1.13.1
  emits (`rawInput.url`, set only for http and https; the toolCall title
  `<protocol> network access to <host>`, which a `socks5Tcp` or `socks5Udp`
  approval carries without any URL; the request `_meta.permission.title`
  `Allow network access?` with the content text `<protocol> access to <host>`;
  a `network.enabled` additional permission), witnessed on plane `net` with gate
  `network · <target>`, and a `fetch` ask; a WIDER HARNESS SANDBOX
  (`rawInput.additionalPermissions`, the `rawInput.permissions` of codex-acp's
  `request_permissions` ask, claude-agent-acp's `dangerouslyDisableSandbox: true`),
  witnessed with gate `sandbox · <widening>`. claude-agent-acp's other Bash keys
  (`command`, `description`, `timeout`, `run_in_background`) and gemini-cli's
  bare asks carry none of these and are judged as before;
- codex-acp's approval of its own MCP tool call (an `execute` ask with no command
  and the request `_meta.is_mcp_tool_approval: true`) is an escalation judged on
  plane `tools` with gate `mcp · <server or tool call>`, never answered by a grant
  because `permits.tools` names the workflow's MCP tools, not the harness's
  (claude-agent-acp and gemini-cli ask theirs as kind `other`, which pauses too).

**What the bridge does not cover**, stated by `nika check` as the `[harness]`
hint and in [docs/usage/harness-agents.md](../usage/harness-agents.md): whatever
the held mode lets the harness do without asking (the table); an allowed action
runs under the harness's own sandbox, not Nika's exec sandbox, so `permits.fs` and
`permits.net` do not confine it, and the connections an allowed command opens by
itself are not asks (Codex keeps sandboxed commands offline; Claude Code and
Gemini CLI commands have the host's network); what Codex does on its own (its MCP
tools marked `readOnlyHint`, its hosted web tool, its `~/.codex`, the MCP servers it
starts and the user's global `AGENTS.md` and plugins it applies, measured on
2026-10-26); on every seat but Claude Code (1bis), the harness's own permission
rules, hooks and configured MCP servers
act without Nika's judgment unless they ask; an unmeasured seat is judged on its
asks alone. `NIKA-DRIFT-001` never calls the `permits.exec` or `permits.fs` grants
of such a run dead: the bridge judges asks against them.

Negative tests: `crates/nika-acp/tests/gate_e2e.rs` and `confine_e2e.rs` (real
spawned harness), `crates/nika-harness/src/client/confine/tests.rs` (scripted
peers), `crates/nika-runtime/src/tests/harness_confinement.rs` (the journal).

## 7 · Billing honesty (P3.9)

Harness rows emit `access=harness` · `billing=included_quota|extra_usage|unknown`
· `UnpricedReason::SubscriptionQuota` — never a fabricated $0 (A-6/A-7). The
ledger law is untouched (`unmetered never trips` · `ledger.rs:190`); the
epilogue speaks the quota line only when observable.

## 7bis · Visible thinking (2026-10-09)

Silence is the transport's signal only when the agent is genuinely silent. Measured on Claude
Code ACP authoring calls (`@agentclientprotocol/claude-agent-acp` 0.81.1, effort `max`): no
thought frame ever arrived, so a call that thought for minutes wrote only its answer chunks at
the end, and a call still thinking at the silence allowance was cut with nothing but one status
and one usage frame. The adapter writes `agent_thought_chunk` only for non-empty thinking
text, and recent models default the SDK's `thinking.display` to `omitted` (signature-only
blocks whose text is empty). Codex ACP streams its reasoning unchanged.

So every Claude Code session the engine opens, the audited one-shot profile (authoring,
`infer:`) and the `agent:` seat alike, sends `_meta.claudeCode.options.thinking =
{type: adaptive, display: summarized}` with `session/new`: the adapter spreads those options
over its defaults, and the SDK maps them to `--thinking adaptive --thinking-display
summarized`. Adaptive thinking is the SDK's default for the models that support it, so this
changes what is displayed, never the model, the effort, a budget or a tool; an agent session
carries it beside the options of §6 (1bis), and its own loop and tools stay its own. A
thinking option a caller already names is kept as given. Codex and every other adapter send
no options. Thought frames stay counted, never stored as text, and the 600 s silence
allowance and the frames that re-arm it are unchanged: a thought frame shows the adapter
wrote one, not what the model is doing. A thinking setting of the user's own Claude Code
configuration reaches neither profile (1bis). Whether every offered Claude model accepts
adaptive thinking is to be verified live on the integrated binary.

## 7ter · Persistent conversation profile (2026-10-09)

A Session conversation led by an ACP agent runs in one persistent agent session whose only tools
are Nika's own (`nika_harness::conversation`, opened by `SpawnedHarness::converse`). It sits
beside the one-shot profile, which is unchanged.

- **Once, then per entry.** `session/new` once per conversation, one `session/prompt` per user
  entry, request ids from 101; the agent keeps the history and runs its own loop until it answers.
  The session's root is a fresh scratch directory; the adapter's process group, the scratch and
  the driver end with the `Conversation` handle.
- **Closed surface, no turn limit.** Claude Code receives the one-shot's audited options without
  `maxTurns`: `tools: []`, `mcpServers: {}`, `strictMcpConfig: true`, `settingSources: []`, no
  plugins, skills or agents, `allowDangerouslySkipPermissions: false`, `persistSession: false` and
  summarized thinking. Its only MCP server is Nika's, mounted through `session/new`'s
  `mcpServers` (HTTP: `type`, `name`, `url` and an `Authorization` bearer header). Codex mounts
  Nika's server from its own configuration (`mcp_servers.<name>`, every other configured server
  disabled, every tool feature off, read back with `codex mcp list` before the spawn: exactly one
  server enabled, Nika's), reads the bearer from the environment variable the configuration
  names (`NIKA_MCP_BEARER`, never the configuration or a command line), and keeps the `read-only`
  mode applied and read back. A user server already named like Nika's refuses the conversation.
- **Admission and transport.** Only claude-agent-acp 0.81.1 and codex-acp 1.13.1 are admitted,
  judged on `initialize` as the one-shot judges them. The transport follows the agent: HTTP when
  Claude Code advertises `agentCapabilities.mcpCapabilities.http` (Codex's configuration speaks
  Streamable HTTP) and Nika offers its endpoint, else Nika's stdio bridge when offered;
  otherwise the conversation is refused after `initialize`, before any session, and nothing falls
  back to the one-shot path. The opening record names the transport chosen, what the agent
  advertised, what was offered, the server and the selection applied; never the endpoint or the
  bearer.
- **Permissions.** Nika answers every `session/request_permission` itself: `allow_once` only when
  the call names one of the offered tools on Nika's server (`mcp__<server>__<tool>`, read from
  `_meta.claudeCode.toolName` or the call's `name`, the server Claude Code names beside it
  agreeing); anything else is rejected with the first `reject_once` offered, after which the
  agent goes on without it or the harness ends the turn, as that option does (`cancelled` when
  no such option is offered). `allow_always` and remembered rejections are never chosen. Each
  decision is a turn beat and lands in the turn record (`allowed` count, `denied` tool names,
  plain words only).
- **Turns and Stop.** A turn's beats are its answer and thought text, its tool frames (now
  expected activity) and the permission decisions, ending with its record: the closed end
  (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`, `other`), the Stops
  received and whether `session/cancel` went out (once per turn, however many Stops), the
  permissions, the bounds and the activity of §7bis's records. One turn runs at a time; a prompt
  sent mid-turn is refused on its own stream. The turn's silence allowance applies while no tool
  call is open (Nika's tool time is the Session's, bounded by its Stop), and again once Stop was
  sent; between turns there is no bound. A transport failure, an unreadable line or a turn that
  outlives its allowance ends the conversation.
- **Not yet.** `session/load` is not used: a reopened conversation starts a new session and the
  Session folds its context brief into the first prompt. The stdio bridge exists as a library
  (`nika_mcp::conversation::bridge`) but no CLI starts it yet (`nika mcp --session`), so an agent
  without HTTP MCP is refused. How codex-acp 1.13.1 presents a Nika tool call (whether it asks a
  permission, and under which name) and the real Claude adapter's frames are to be confirmed by a
  live probe.

## 8 · Non-goals (P3)

- No `infer:` on a harness (P4 · capability attestation first).
- No model selection — the resolver never picks a model (A-1/A-2 · H8).
- No new workflow YAML key · no spec change · no schema edit (through P5).
- No oauth flows (P5 · xAI device-flow pilot).
- No parallel harness sessions (serial self-queue; revisit on evidence).

## 9 · Gates plan

Gate 5 mutation ≥90% on the session state machine + the permission bridge ·
Gate 6 proptest on update-stream framing (bounded reads · interleaving) ·
Gate 9 canary = the mock-agent e2e (hermetic) · Gate 10 N/A (no legacy ACP) ·
admission ceremony full (new crate).

🦋
