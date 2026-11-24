# Job anatomy

Read this reference for the matching task; return to [the skill](../SKILL.md) for scope and completion.

A real job is not « read, ask a model, write ». It gathers from its sources,
makes the data trustworthy, decides what deserves attention, acts only on
that, and says what it did. Write the design first, then build the richest
honest version: craft additions by default, effect additions only when the
person accepts them ([capabilities](capabilities.md)).

## The five phases

| Phase | What it does | Typical builtins |
|---|---|---|
| Gather | read every source, in parallel when they are independent; a dead source yields a recover sample, not a dead run | `nika:fetch` modes, `nika:read`, `nika:glob`, `nika:grep`, `exec:`, `mcp:` |
| Normalize | turn text into typed data and refuse what does not fit | `nika:convert`, `nika:jq`, `extract:`, `nika:validate`, `nika:assert`, `nika:hash`, `nika:date` |
| Judge | decide what matters: compare with last run, apply laws, let a model judge only what needs language | `nika:json_diff`, `when:`, `nika:decide`, `infer:` with `schema:` and `max_tokens:`, a bounded `agent:` |
| Act | write, edit, draw, send, each effect gated on the data that justifies it | `nika:write`, `nika:edit`, `nika:chart`, `nika:image_generate`, `nika:notify` |
| Report | build the run report as a value, write it, return it | `nika:inspect` `view: cost`, `nika:jq`, `nika:write`, `nika:emit`, `outputs:` |

## The depth bar

Before handing a job over, it clears this bar or the hand-over says why not:

- **Real steps.** Each task changes data or makes a decision. A task that only
  logs is not a step.
- **Branch points.** At least one `when:` that skips work on a quiet run, and
  a failure route: an `on_error:` with `on_codes`, or an `after: { <task>: failure }`.
- **A join.** At least one task binds two or more upstream outputs, or folds a
  `group:`.
- **Breadth of tools.** At least eight distinct builtins when the job touches
  data. Fewer means a phase is missing; name which one and why.
- **State** when the job repeats: it reports what is new since the last run
  (offered, not added, when the job fetches: see the first pattern).
- **A run report**: what was read, what was decided, what was written, what it
  cost.
- **At most one human gate**, and none at all where nothing leaves the
  machine or cannot be undone (« Approve once, then let it run »).

Calibrate on the deep examples: `release-radar` (state and a root gate),
`config-drift-sentinel` (fingerprint, merge patch, diff, triage),
`competitor-radar` (date window, bounded fan-out, zip join), `ceo-monday-brief`
(three-branch gather, one gate, capped synthesis).

## Patterns

Each snippet comes from the named example, trimmed to the pattern; the
idempotency and run-report snippets are compositions measured on the engine.
Every one was checked inside a complete workflow. Read the example for the
envelope around it.

### State between runs

From `release-radar`. One path on both sides: the same literal is in
`fs.read` and `fs.write`.

```yaml
  previous:
    invoke:
      tool: "nika:read"
      args: { path: "${{ const.state_path }}" }
    on_error:
      on_codes: [NIKA-BUILTIN-READ-001]   # not-found only: the first run has no state yet
      recover: []
  previous_parsed:
    with: { previous: "${{ tasks.previous.output }}" }
    invoke:
      tool: "nika:jq"   # a read returns a string: parse it, keep the first-run []
      args: { input: "${{ with.previous }}", expression: 'if type == "string" then fromjson else . end' }
  fresh:
    with:
      previous: ${{ tasks.previous_parsed.output }}
      entries: ${{ tasks.feed.entries }}
    invoke:
      tool: "nika:json_diff"   # RFC 6902: the additions are what is new
      args: { before: "${{ with.previous }}", after: "${{ with.entries }}" }
  save_state:
    with: { entries: "${{ tasks.feed.entries }}" }
    after: { fresh: success }
    invoke:
      tool: "nika:write"
      args: { path: "${{ const.state_path }}", content: "${{ with.entries }}", create_dirs: true, overwrite: true }
```

Keep the state file in the folder the person named for output; when they
named none, propose its path in one line. A state file plus a fetch completes
the lethal trifecta (the state read is a private read, the fetch is untrusted
ingress, `net.http` is egress): check refuses `NIKA-SEC-009` until one
confirm gate sits at the root (the shape is below, in « A fetching job that
keeps state »), and that gate asks at every run. So for a fetching job, offer
state in one line instead of adding it by default. A job whose sources are
all local files keeps state with no gate.

### Idempotency and dedupe

A retried `POST`, `PUT`, `PATCH` or `DELETE` must carry an `idempotency-key`
header: without it check refuses the `retry:` (`NIKA-SEC-016`). Derive the key
from the content with `nika:hash`, as `config-drift-sentinel` fingerprints a
response (stdlib §`nika:fetch`, the effect-safe retry law):

```yaml
  key:
    invoke:
      tool: "nika:hash"
      args: { algo: sha256, content: "${{ const.ticket }}" }
  open:
    with: { key: "${{ tasks.key.output }}" }
    retry: { max_attempts: 3, backoff_strategy: exponential }
    invoke:
      tool: "nika:fetch"
      args:
        url: "https://api.example.com/tickets"
        method: POST
        headers: { idempotency-key: "${{ with.key }}" }
        body: "${{ const.ticket }}"
```

Dedupe records before acting on them, and refuse conflicting duplicates
instead of keeping the first (`21-deduplicate-records`):

```yaml
  deduplicate:
    after:
      agree: success
    invoke:
      tool: nika:jq
      args:
        input:
          dataset: ${{ const.dataset }}
          records: ${{ const.records }}
        expression: '{dataset, records: (.records | unique_by(.id))}'
```

### Validation of external and model values

From `24-recover-optional-file`: validate, then assert the verdict, then use
the value. Give every model answer that is data a `schema:` and a
`max_tokens:`; an answer that does not fit fails instead of flowing on.

```yaml
  validate:
    with:
      content: ${{ tasks.read.output }}
    invoke:
      tool: nika:validate
      args:
        data: ${{ with.content }}
        format: json
        schema:
          type: string
          minLength: 1
          maxLength: 4000
  admit:
    with:
      valid: ${{ tasks.validate.output.valid }}
    invoke:
      tool: nika:assert
      args:
        condition: ${{ with.valid }}
        message: Content must be nonempty and at most 4000 characters
```

### Budget assertion

When the risk is cost, assert the bound instead of asking a person. From
`18-bounded-batch`: refuse an oversized batch before any paid call, then fan
out with a pool and a ceiling per item.

```yaml
  process:
    after:
      admit: success
    for_each:
      items: ${{ const.items }}
      max_parallel: 2
      fail_fast: true
    timeout: 30s
    infer:
      prompt: '${{ const.brief }}

        Item: ${{ item }}'
      max_tokens: 2048
      schema:
        type: object
        additionalProperties: false
        required:
        - summary
        properties:
          summary:
            type: string
            minLength: 1
            maxLength: 4000
```

`for_each.max_items` refuses a longer collection before its first item, and
`nika.yaml` `ceiling:` or `--max-cost-usd` caps the run. In a job that
writes, every `nika:assert` carries the advisory `assert-quarantine` hint
(see « Two hints that cannot both clear »).

### Run report

Build the report as a value with `nika:inspect` (`16-inspect-self`). Its cost
view is `{ total_usd, by_task, by_provider, metered }`: `metered: false` means
nothing was priced (a local or subscription model), never that it was free.

```yaml
  cost:
    after: { summary: terminal }
    invoke: { tool: "nika:inspect", args: { view: cost } }
  within_budget:
    with: { cost: "${{ tasks.cost.output }}" }
    invoke:
      tool: "nika:assert"
      args: { condition: "${{ with.cost.total_usd <= const.max_usd }}", message: "the run spent more than its budget" }
  report:
    with:
      cost: ${{ tasks.cost.output }}
      summary: ${{ tasks.summary.status }}
    invoke:
      tool: "nika:jq"
      args:
        input: { cost: "${{ with.cost }}", summary: "${{ with.summary }}" }
        expression: "{ spent_usd: .cost.total_usd, metered: .cost.metered, steps: { summary: .summary } }"
```

Return the report in `outputs:`, and write it with `nika:write` inside the
folder the person named (a write elsewhere is an effect to propose). Give the
writer `after: { <last task>: terminal }` so the report survives a failed run
(« The always-run report » below).

### Fan-out and join

From `competitor-radar`: a bounded pool where one dead item yields `null`,
then a join that zips the inputs with the results and drops the failures.

```yaml
  pages:
    with:
      urls: ${{ tasks.recent.output }}
    for_each:
      items: ${{ with.urls }}
      max_parallel: 4
      fail_fast: false
    on_error:
      recover: null
    timeout: "30s"
    retry:
      max_attempts: 3
      backoff_strategy: exponential
      jitter: true
    invoke:
      tool: "nika:fetch"
      args:
        url: "${{ item }}"
        mode: article
  readable:
    with:
      urls: ${{ tasks.recent.output }}
      pages: ${{ tasks.pages.output }}
    invoke:
      tool: "nika:jq"
      args:
        input: ["${{ with.urls }}", "${{ with.pages }}"]
        expression: >-
          transpose
          | map(select(.[1] != null))
          | map({ url: .[0], text: .[1] })
```

### Recover samples

From `release-radar`: with no network, the recovery stands in for the RAW
response, in the shape the mode returns, so the `extract:` binding runs
unchanged. Recovering the binding's shape instead fails `NIKA-VAR-004`.

```yaml
  feed:
    invoke:
      tool: "nika:fetch"
      args:
        url: "${{ const.releases_feed }}"
        mode: feed
    extract:
      entries: "[.items[] | {title, link}]"
    on_error:
      recover:
        items:
          - { title: "tokio 1.48.0", link: "https://github.com/tokio-rs/tokio/releases/tag/tokio-1.48.0" }
```

### The always-run report

A failed run moves every file its writers created during that run to
`.nika/quarantine/<trace>/`, whatever failed: a red `nika:assert`, a dead
source, a refused call. A file that existed before the run never moves. The
exception is a writer admitted by an `after:` edge on `failure`, `terminal`
or `unwind`: its file is the point of that path and stays. So the report's
own writer carries `after: { <last task>: terminal }`. A report reached only
through data from such a task is quarantined with the rest (measured).

What the report may bind: a task that failed or was blocked blocks every
reader of its `output`; its `status` passes whether it succeeded or failed.
Its `error` passes only when it failed: binding the `error` of a task that
succeeded blocks the report. Read the outcome of each task from
`nika:inspect` `view: records`.

```yaml
nika: daily-count
permits:
  tools: ["nika:read", "nika:jq", "nika:write", "nika:inspect"]
  fs:
    read: ["./in/data.json"]
    write: ["./out/summary.json", "./out/run-report.json"]
tasks:
  source:
    invoke:
      tool: "nika:read"
      args: { path: "./in/data.json" }
  summary:
    with: { raw: "${{ tasks.source.output }}" }
    invoke:
      tool: "nika:jq"
      args: { input: "${{ with.raw }}", expression: "fromjson | length" }
  save:
    with: { n: "${{ tasks.summary.output }}" }
    invoke:
      tool: "nika:write"
      args: { path: "./out/summary.json", content: "${{ with.n }}", create_dirs: true, overwrite: true }
  records:
    after: { save: terminal }
    invoke: { tool: "nika:inspect", args: { view: records } }
  report:
    after: { save: terminal }
    with:
      source: ${{ tasks.source.status }}
      records: ${{ tasks.records.output }}
    invoke:
      tool: "nika:write"
      args:
        path: "./out/run-report.json"
        content: { source: "${{ with.source }}", records: "${{ with.records }}" }
        create_dirs: true
        overwrite: true
```

Measured: with `./in/data.json` missing, the run exits 1 and
`./out/run-report.json` stays in place, listing `source` as failed and
`summary` and `save` as cancelled; with the file present, both files are
written. Moving `after:` from `report` to `records` alone sends the report to
quarantine.

### Approve once, then let it run

The person approved the job when they asked for it, and again when they
chose to schedule it. A question at every step trains them to click through
without reading, so a gate is the exception:

- A human gate exists only where something leaves the machine to a person or
  a third party (a message, a post, an upload, a ticket), or cannot be undone
  (a deletion, a payment, a deploy).
- A flow that `nika check` does not refuse with `NIKA-SEC-009` needs no gate.
  When one is needed, prefer `mode: confirm`: one question that names
  everything the run will do.
- Cost is never a gate. Bound it with a `nika:assert` on a computed budget
  (« Budget assertion » above) plus the run ceiling (`--max-cost-usd`, or the
  project's `ceiling:`).
- A scheduled workflow (an `arm:` beat) that keeps a gate pauses at every
  fire on this release (exit 4) until a person answers. Say so before it is
  armed, and design the gate out where the risk allows: keep state on local
  sources, drop a send the job does not need, or write a draft for the
  person instead of sending it.
- Never answer a gate for the person and never pass `--answer` on your own
  initiative. A paused run is shown to the person with its question, and
  resumed only with their answer: `--resume <trace> --answer <task>=<answer>`.

How check counts a gate, measured on this release:

- A `mode: confirm` gate counts by its position: it must stand on every path
  to the effect, data edges included, which in practice makes it the root.
  A refused confirm is the value `false`, not an error, so every effect reads
  the answer: `with: { go: ... }` and `when: ${{ with.go == true }}` (check
  raises a `consent` hint on an effect whose reading it cannot prove).
- A `mode: choice` or `mode: input` gate counts only before an effect whose
  `when:` tests its answer (`when: ${{ with.pick == 'send' }}`); the same
  choice gate without that `when:` is refused `NIKA-SEC-009`.
- A `with:` data edge is a path. The gate must sit upstream of every task
  that untrusted content reaches through data, not only of the task that
  writes: a gate placed after a fetch, with a data edge from the fetch
  straight to the write, does not dominate it and is refused.
- State written by one run reaches the next run's reads through the file, so
  the state file holds what the last run fetched. Its read stays behind the
  gate too: the same job with the state read before the gate is refused
  `NIKA-SEC-009`.
- A gate with a `default:` answers itself unattended and is not a gate. A gate
  with `on_error:` or `for_each:` counts before no effect.

### A fetching job that keeps state

State between runs plus a fetch completes the lethal trifecta: the state read
is a private read, the feed is untrusted ingress, `net.http` is egress. Check
refuses that file `NIKA-SEC-009` until one confirm gate stands at the root.
Say it plainly to the person: this job asks once per run, and a scheduled run
waits for them. The gate's answer reaches every effect:

```yaml
nika: release-watch
const:
  releases_feed: "https://github.com/tokio-rs/tokio/releases.atom"
  state_path: "out/release-watch.json"
permits:
  tools: ["nika:prompt", "nika:fetch", "nika:read", "nika:jq", "nika:json_diff", "nika:write"]
  net:
    http: ["github.com"]
  fs:
    read: ["out/release-watch.json"]
    write: ["out/release-watch.json"]
tasks:
  approve:
    invoke:
      tool: "nika:prompt"
      args:
        mode: confirm
        message: "This run reads ${{ const.state_path }}, fetches ${{ const.releases_feed }} and rewrites the state file. Proceed?"
  previous:
    with: { go: "${{ tasks.approve.output }}" }
    when: ${{ with.go == true }}
    invoke:
      tool: "nika:read"
      args: { path: "${{ const.state_path }}" }
    on_error:
      on_codes: [NIKA-BUILTIN-READ-001]
      recover: []
  feed:
    with: { go: "${{ tasks.approve.output }}" }
    when: ${{ with.go == true }}
    invoke:
      tool: "nika:fetch"
      args: { url: "${{ const.releases_feed }}", mode: feed }
    extract:
      entries: "[.items[] | {title, link}]"
  previous_parsed:
    with: { previous: "${{ tasks.previous.output }}" }
    invoke:
      tool: "nika:jq"
      args: { input: "${{ with.previous }}", expression: 'if type == "string" then fromjson else . end' }
  fresh:
    with:
      previous: ${{ tasks.previous_parsed.output }}
      entries: ${{ tasks.feed.entries }}
    invoke:
      tool: "nika:json_diff"
      args: { before: "${{ with.previous }}", after: "${{ with.entries }}" }
  save_state:
    with:
      go: ${{ tasks.approve.output }}
      entries: ${{ tasks.feed.entries }}
    when: ${{ with.go == true }}
    after: { fresh: success }
    invoke:
      tool: "nika:write"
      args: { path: "${{ const.state_path }}", content: "${{ with.entries }}", create_dirs: true, overwrite: true }
outputs:
  new_entries: ${{ tasks.fresh.output }}
```

The same file without `approve` is refused `NIKA-SEC-009`; with it, check is
clean with no hint. `release-radar` is the full version of this job.

That root gate asks before anything is listed: the person approves a run
whose findings they have not seen. The honest alternative is a first,
read-only phase that lists what the run will act on, then the gate the
trifecta requires, then the state phase. The preview cannot say what is new,
because comparing with the state is a private read and stays behind the gate,
and the second fetch can differ from the preview. Say both when you offer it.

```yaml
nika: release-watch-preview
const:
  releases_feed: "https://github.com/tokio-rs/tokio/releases.atom"
  state_path: "out/release-watch.json"
permits:
  tools: ["nika:fetch", "nika:jq", "nika:prompt", "nika:read", "nika:json_diff", "nika:write"]
  net:
    http: ["github.com"]
  fs:
    read: ["out/release-watch.json"]
    write: ["out/release-watch.json"]
tasks:
  preview:
    invoke:
      tool: "nika:fetch"
      args: { url: "${{ const.releases_feed }}", mode: feed }
    extract:
      entries: "[.items[] | {title, link}]"
  listing:
    with: { entries: "${{ tasks.preview.entries }}" }
    invoke:
      tool: "nika:jq"
      args: { input: "${{ with.entries }}", expression: '"\(length) releases in the feed: " + (map(.title) | join(", "))' }
  approve:
    with: { listing: "${{ tasks.listing.output }}" }
    invoke:
      tool: "nika:prompt"
      args:
        mode: confirm
        message: "${{ with.listing }}. Compare them with ${{ const.state_path }} and rewrite that file?"
  feed:
    with: { go: "${{ tasks.approve.output }}" }
    when: ${{ with.go == true }}
    invoke:
      tool: "nika:fetch"
      args: { url: "${{ const.releases_feed }}", mode: feed }
    extract:
      entries: "[.items[] | {title, link}]"
  previous:
    with: { go: "${{ tasks.approve.output }}" }
    when: ${{ with.go == true }}
    invoke:
      tool: "nika:read"
      args: { path: "${{ const.state_path }}" }
    on_error:
      on_codes: [NIKA-BUILTIN-READ-001]
      recover: []
  previous_parsed:
    with: { previous: "${{ tasks.previous.output }}" }
    invoke:
      tool: "nika:jq"
      args: { input: "${{ with.previous }}", expression: 'if type == "string" then fromjson else . end' }
  fresh:
    with:
      previous: ${{ tasks.previous_parsed.output }}
      entries: ${{ tasks.feed.entries }}
    invoke:
      tool: "nika:json_diff"
      args: { before: "${{ with.previous }}", after: "${{ with.entries }}" }
  save_state:
    with:
      go: ${{ tasks.approve.output }}
      entries: ${{ tasks.feed.entries }}
    when: ${{ with.go == true }}
    after: { fresh: success }
    invoke:
      tool: "nika:write"
      args: { path: "${{ const.state_path }}", content: "${{ with.entries }}", create_dirs: true, overwrite: true }
outputs:
  new_entries: ${{ tasks.fresh.output }}
```

Measured: check is clean with no hint. Bind the preview into the question
only: the same file where the write also binds the preview's entries is
refused `NIKA-SEC-009`, since that data edge bypasses the gate. The question
shows text a feed chose; it is displayed to the person, never executed.

## Engine behaviors to design around

Each was measured on this release with `nika check`, and with runs where a
run is named. Design around them; none is a defect of your file.

### A secret's taint reaches every value of the call

A call that uses a secret (`nika:notify` to a webhook held in a secret, a
`nika:fetch` with the token in a header) returns values that all carry the
secret's taint: the output, the response and the status alike. Binding any
of them into a write's arguments is refused `NIKA-SEC-006`. To record that
the call happened, gate the downstream task with `when:` alone and keep the
tainted value out of its arguments (`after: { alert: success }` works too):

```yaml
nika: alert-then-record
secrets:
  team_hook:
    source: env
    key: TEAM_WEBHOOK_URL
    egress:
      - to: "nika:notify"
        host_from_self: true
permits:
  tools: ["nika:notify", "nika:write"]
  net:
    http: ["hooks.slack.com"]
  fs:
    write: ["./out/sent.txt"]
tasks:
  alert:
    invoke:
      tool: "nika:notify"
      args: { channel: webhook, target: "${{ secrets.team_hook }}", message: "Nightly report is ready" }
  record:
    with:
      sent: ${{ tasks.alert.status }}
    when: ${{ with.sent == 'success' }}
    invoke:
      tool: "nika:write"
      args: { path: "./out/sent.txt", content: "sent", create_dirs: true, overwrite: true }
```

Keep the secret's `egress:` narrow: one `to:` and, for `nika:fetch`, its host
(`egress: [{ to: "nika:fetch", host: "api.example.com" }]`). The fix line
that adds `nika:write` to `to:` would also clear the finding, and would let
the secret's flow reach the file: never widen it just to read a status.

### Expressions

`${{ }}` and `when:` speak `cel-subset/0.1` (spec 03 §Formal grammar).
Measured:

- Accepted: `==`, `!=`, `<`, `<=`, `>`, `>=`, `in` against a list, `&&`,
  `||`, `!`, parentheses, field access, indexing (`with.items[0]`),
  `size(x)` and `x.size()`, `has(x.field)`, `x.contains(s)`,
  `x.startsWith(s)`, `x.endsWith(s)`, a comparison with `null`, and the
  conditional `c ? a : b`.
- Roots in `when:`: `with.`, `inputs.`, `const.`. A `tasks.` reference there
  is `NIKA-VAR-021`: bind it with `with:` first.
- Refused at check, `NIKA-VAR-005`: arithmetic (`+`, `-`, `*`, string
  concatenation included), the macros `map()` and `all()`, `matches()` and
  every other method (`lowerAscii()`), a chained relation (`1 < with.n < 5`),
  a bare reference (`when: ${{ with.flag }}`: write `with.flag == true`).
- Refused at run, `NIKA-VAR-006`: `in` against a map (`'k' in with.obj`).
  Test a key with `has(with.obj.k)`.
- Quote a value that holds a conditional: unquoted, YAML reads `a ? b : c` as
  a mapping (`NIKA-PARSE-001`).

Arithmetic belongs to `nika:jq`: compute the number or the boolean there,
bind it with `with:`, compare it in `when:`. A time span is a duration
string (`timeout: "90s"`, `nika:date` `duration: "P1D"`), never a product.

### Field and tool limits

| Limit, as measured | Workaround |
|---|---|
| `nika:image_fx` `ops:` from `${{ }}` is refused `NIKA-BUILTIN-001` | write the op list literally in the task |
| YAML anchors and aliases (`&say`, `*say`) are refused `NIKA-PARSE-001` | repeat the block |
| `max_tokens:` from `${{ }}` is refused `NIKA-PARSE-019` | a literal number in the task |
| `infer.schema:` from `${{ const.x }}` is refused `NIKA-PARSE-019` | write the schema out in the task, and again in `nika:validate` when both use it |
| jq has no `\p{…}` class: `test("\\p{L}")` is refused `NIKA-VAR-005` | a code-point range: `test("[\\x{0590}-\\x{05FF}]")` works |
| jq `label` is a keyword: `def label: …` is refused `NIKA-VAR-005` | name the function otherwise; `.label` and `{label: .}` work |
| `nika:date` refuses a bare date (`base: "2026-11-24"`) at run, `NIKA-BUILTIN-DATE-001` | a full RFC 3339 timestamp (`2026-11-24T00:00:00Z`), or `op: parse` with `format: "%Y-%m-%d"` first |
| `nika:chart` `semantics:` knows one money grain, `usd`; `eur` is refused | convert with `nika:jq` and label the axis, or use `count` |
| a `nika:decide` receipt is one object | route a lane on `with.receipt.outcome` in `when:`; derive anything finer (a dimension, a list) with `nika:jq` first |

### Downloading images and other binary files

`nika:fetch` refuses a body that is not text: fetching an image in
`mode: raw` fails `NIKA-BUILTIN-FETCH-001` (« response body is not valid
UTF-8 »), measured. Binary is file-mediated: an `exec:` of `curl --fail
--location --output <path> <url>`, with `permits.exec: ["curl"]`, the host
in `net.http` and the path in `fs.write`, then the path feeds `nika:image_fx`,
`nika:image_compose` or `infer.vision`. Plain check admits it;
`--native-strict` flags it `native-first/001`, pointing at `nika:fetch`,
which cannot do it. That flag cannot clear on this release: say so in the
hand-over instead of looping. An MCP tool that returns image content also
works: its bytes flow into `nika:write` `content:`.

### A skipped write leaves the last run's file

`when:` skips a task; it does not undo what an earlier run wrote. An alert
written on Monday is still there on a quiet Tuesday, and a reader takes it
for today's. Two ways out:

- Always write the report, with no `when:`, and let its content say that
  nothing is new.
- Remove the stale artifact on the quiet branch with `nika:remove_file`,
  under the same `fs.write` bound. Order the removal after the skipped write
  (`after: { alert_file: skipped }`): two unordered tasks on one path are
  refused `NIKA-SEC-012`. Removing a missing file fails
  `NIKA-BUILTIN-REMOVE_FILE-002`; recover it for the quiet run that finds
  nothing to remove.

```yaml
nika: alert-file
inputs:
  alerts:
    type: { array: string }
    required: false
    default: []
permits:
  tools: ["nika:jq", "nika:write", "nika:remove_file"]
  fs:
    write: ["./out/alert.md"]
tasks:
  count:
    with:
      alerts: ${{ inputs.alerts }}
    invoke:
      tool: "nika:jq"
      args: { expression: "length", input: "${{ with.alerts }}" }
  alert_file:
    with:
      n: ${{ tasks.count.output }}
      alerts: ${{ inputs.alerts }}
    when: ${{ with.n > 0 }}
    invoke:
      tool: "nika:write"
      args: { path: "./out/alert.md", content: "alerts: ${{ with.alerts }}", create_dirs: true, overwrite: true }
  clear_alert:
    after: { alert_file: skipped }
    on_error:
      recover: "absent"
      on_codes: ["NIKA-BUILTIN-REMOVE_FILE-002"]
    invoke:
      tool: "nika:remove_file"
      args: { path: "./out/alert.md" }
```

Measured: a run with an alert writes the file, the next quiet run removes it,
and a third quiet run recovers the missing file. Without `clear_alert`, the
quiet run leaves Monday's alert in place.

### Rehearsing with mock

`--model mock/echo` answers without a model, measured: plain text comes back
as `mock(echo) · <the prompt>`; a typed answer takes the first `enum` value,
`0` for an integer, `false` for a boolean and `"mock"` for a string. So:

- List the safe value first in every enum (`enum: ["hold", "send"]`), so a
  rehearsal takes the conservative branch.
- Prove each law on hand-known fixtures with `nika:assert`
  (`13-extract-then-law`): the law's branches then run with no model.
- For a branch that needs a particular answer, rehearse a scratch copy where
  the `infer:` task becomes a `nika:jq` returning a fixture of the same shape;
  check it, run it, delete it. `nika test --case <name> --var <key>=<value>`
  pins each input-driven case in its own golden.
- Mock removes inference only: the file's tools, writes and fetches are real.

### Resume

`nika run <file> --resume <trace>` skips the tasks the trace settled. A trace
with no resume keys (measured on a run whose only task failed) says it
« carries no resume keys » and is « running everything live »: every task
runs again, effects included. Read that line before resuming a run that wrote or
sent something.

### Two hints that cannot both clear

When a model answer feeds a `nika:jq` or `nika:decide` law and the job
writes, check raises `unproven-law` until the law is proved on a hand-known
fixture with a `nika:assert` (`13-extract-then-law`). That assert then raises
`assert-quarantine`, which fires for any `nika:assert` in a workflow that
writes, wherever it sits. On this release the two cannot both clear. It is a
known engine limit, not a defect in the file: clear `unproven-law`, which
blocks `paid_ready`, and read `assert-quarantine` as advice, since it blocks
neither `clean` nor `paid_ready` (a red assert moves the run's writes to
`.nika/quarantine/<trace>/`). Name it once in the hand-over and stop there.

## Before handing over: the thin-workflow checklist

Count and report: tasks, distinct builtins, branch points, joins, state, the
report, gates. Then answer each line, or say in the hand-over why it does not
apply:

- [ ] More than three tasks, and more than one phase. A read, infer, write
      chain is the thin shape.
- [ ] Every external value (a file, a page, an API answer, a model answer that
      is data) is validated before an effect or a model uses it.
- [ ] Every `infer:` has `max_tokens:`, and `schema:` when its output is data.
      Every `agent:` has `max_turns:` and `max_tokens_total:`.
- [ ] A quiet run spends nothing: `when:` on `size(...) > 0` before paid work.
- [ ] A repeated job keeps state and reports only what is new (a fetching
      job offers it, since it brings the gate).
- [ ] Every fan-out has `max_parallel`; a batch that must survive one bad item
      has `fail_fast: false` and a `recover:`.
- [ ] Every source that can fail has a recover sample in the raw shape.
- [ ] Every retried mutating call carries an `idempotency-key`.
- [ ] A run report exists as a value, written and returned, by a writer with
      `after: { <last task>: terminal }` so a failed run keeps it.
- [ ] Every enum lists its safe value first (mock takes the first).
- [ ] A write behind `when:` either always writes, or removes its stale file
      on the quiet branch.
- [ ] No send's reply feeds a write: the record is gated by `when:` alone.
- [ ] At most one gate, only where something leaves the machine or cannot be
      undone and check or the person requires it; none on cost. A scheduled
      job that keeps its gate is said to pause at every fire.
- [ ] No effect the person did not ask for: a new send, host, paid model, seat
      route or write outside their folder is proposed in one line instead.
- [ ] `nika check <file> --json --native-strict` is clean on the final file.
