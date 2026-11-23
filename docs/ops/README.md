# nika serve operations

`nika serve` is a resident. Bare `nika serve` is the ARM firer: it fires
the schedules ("beats") declared under `arm:` in the project's `nika.yaml`
([arm reference](https://docs.nika.sh/reference/arm)). With `--bind`,
`--workflows` and `--token-file` the resident opens an authenticated HTTP
door on loopback. Either way it runs inside a project: the folder needs a
`nika.yaml` naming it, and one line is enough.

## First run of the HTTP door

```sh
test -f nika.yaml || printf 'nika: my-project\n' > nika.yaml   # an existing one is kept
mkdir -p workflows
test -f workflows/hello.nika || cat > workflows/hello.nika <<'EOF'
nika: hello
model: mock/echo
permits: {}
tasks:
  greeting:
    infer:
      prompt: "Say hello from nika serve."
      max_tokens: 32
outputs:
  greeting: ${{ tasks.greeting.output }}
EOF
nika check workflows/hello.nika
umask 077
mkdir -p .nika/serve
openssl rand -hex 24 > .nika/serve.token
chmod 600 .nika/serve.token
nika serve --bind 127.0.0.1:8787 --workflows ./workflows --token-file .nika/serve.token
```

`--workflows` names a folder that must exist: `nika serve` exits 1 when it
cannot open it. It serves the `.nika` files under that folder, each named
by its path from the project root, here `workflows/hello.nika`; the
rehearsal model `mock/echo` needs no key and no network. `nika serve` also
creates `.nika/serve` (mode 0700) when the default state root is missing.
The mkdir above is the operator-visible first run.
`nika init` adds only `.nika/traces/` to `.gitignore`: add
`.nika/serve.token` and `.nika/serve/` yourself, so neither the token nor
the job state is committed.

From a second terminal, check that it answers, then start a job by its
served name (the workflow's path from the project root):

```sh
curl -s http://127.0.0.1:8787/health
curl -s -X POST http://127.0.0.1:8787/v1/jobs \
  -H "Authorization: Bearer $(cat .nika/serve.token)" \
  -H 'Idempotency-Key: first-run' \
  -H 'Content-Type: application/json' \
  -d '{"workflow":"workflows/hello.nika"}'
```

`/health` needs no token and lists the server's `supportedCapabilities`.
A new job answers `202` with its id and `"status":"queued"`; read it with
`GET /v1/jobs/{id}`. Every other route needs the Bearer token, and
`POST /v1/jobs` also needs an `Idempotency-Key` and a JSON body.
`GET /v1/openapi.json` describes the live routes. Manual jobs run under a
per-run spend ceiling of USD 1 unless `--run-cost-ceiling <USD|none>` says
otherwise; `nika serve --help` lists the optional doors (`--sessions`,
`--cost-review`, a compile seat) and `--cost-policy`
([below](#runs-on-a-route-the-catalog-does-not-price)). The
[server runbook](https://docs.nika.sh/sdk/operations/server-runbook) and
the TypeScript SDK cover the API from an application.

- `POST /v1/jobs/{id}/cancel` cancels a queued job (`200`, settled
  `cancelled`) or asks a running one to stop (`202`, settled later). A job
  already settled or paused answers `200` with its record, unchanged. A
  running job gets a fixed 5-second grace to reach a wave boundary and
  settle; past it the resident drops the run and the job settles
  `interrupted`. A task inside one long step (an agent turn, a long wait)
  usually outlasts the grace.
- `GET /v1/access-offers/{name}` reads what the sessions of a served
  workflow's own declared ACP route offer now (its models, reasoning efforts
  and service tiers), without sending a prompt. `{name}` is the served name,
  as for `GET /v1/workflows/{name}`. A workflow that declares no
  `run.access.via` under `protocol: acp` answers `not_applicable` and opens
  no session. Rows are kept 60 seconds, one reading runs at a time, and the
  answer authorizes no run: see
  [Asking a seat what it offers](../usage/harness-agents.md#asking-a-seat-what-it-offers).
- `GET /v1/jobs/{id}/artifacts` lists what a settled job's journal records
  as its artifacts: the files `nika:chart`, `nika:image_generate`,
  `nika:tts_generate`, `nika:image_fx` and `nika:image_compose` saved inside
  the project, and the
  images an agent's harness sent. Each entry has a `name`, `kind`, `size`,
  `sha256` and `mime`, and the manifest carries the job's `receipt`. Files
  written by `nika:write` or `nika:edit` are not listed, because their
  journal frame records no digest. To serve a file through this route, save
  it with one of those builtins.
- `GET /v1/jobs/{id}/artifacts/{name}` streams one of them, `{name}` being
  the entry's `name`, its `/` sent as is or as `%2F`. The bytes come with
  `Content-Type`, `Content-Length` and `Nika-Artifact-Sha256`. The server
  refuses to serve bytes that no longer match the journal
  (`409 artifact_changed`), and stops a stream at the first chunk that
  changed while it was being sent. A body shorter than its `Content-Length`
  is an incomplete download, also left by a resident that stops mid-stream:
  retry it, and a file that really changed then answers `409`.

Both artifact routes need a sealed journal. Create the resident's run key
with `NIKA_KEYCHAIN=off nika key init`, which keeps it under `~/.nika/keys`.
To keep it elsewhere, set both `NIKA_RUN_KEY_FILE` and `NIKA_RUN_PUB_FILE`
to paths outside it, for `nika key init` and then for `nika serve`. The
routes verify the seal against the keys `nika trace verify` weighs in the
resident's environment, in the order the resident signs: that pair's public
half, `~/.nika/keys/run-signing.pub`, then the retired-key ledger. A journal
left unsealed (no run key) answers `409 artifact_journal_unverified`, as
does one sealed with a key none of those hold. The job must have settled:
a queued, running or paused job answers `409 job_not_settled`. One artifact
is served up to 256 MiB; a larger one stays listed and its download answers
`403 artifact_too_large`.

```sh
TOKEN="$(cat .nika/serve.token)"
curl -s -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8787/v1/jobs/<id>/artifacts
curl -s -H "Authorization: Bearer $TOKEN" -o totals.svg \
  http://127.0.0.1:8787/v1/jobs/<id>/artifacts/file/out/totals.svg
```

## Keeping the job store bounded

The resident keeps every job it admits in one state file under the state
root. Out of the box nothing ever leaves it: after 10,000 jobs every
admission answers `507 job_capacity`, and each operation costs more as the
file grows. A resident that takes steady traffic sets a retention window:

```sh
nika serve --bind 127.0.0.1:8787 --workflows ./workflows \
  --token-file .nika/serve.token \
  --job-retention 24h --job-retention-unsuccessful 7d
```

- `--job-retention` forgets a `succeeded` job once it has been settled that
  long. `--job-retention-unsuccessful` does the same for `failed`,
  `cancelled` and `interrupted` jobs; without it they follow the first
  window. A duration is a whole number and one unit: `s`, `m`, `h` or `d`.
  The minimum is one minute.
- A job that is `queued`, `running` or `paused` is never forgotten.
- Jobs leave when the next job is admitted, and at startup. A read never
  evicts.
- A forgotten job's `Idempotency-Key` is forgotten with it: the same key
  then creates a new job. Keep the window longer than your callers retry.
- `GET /v1/jobs/{id}` and the artifact routes answer `404` for a forgotten
  job. Its trace journal under `.nika/traces` is not removed: it is the
  evidence of the run, and `nika trace` still reads it.

Both flags work without `--bind`: scheduled runs fill the same store.

## Sharing the workers between users

A resident that runs jobs for many users of one application states who each
job is for (`owner` on `POST /v1/jobs`) and bounds what one of them holds:

```sh
nika serve --bind 127.0.0.1:8787 --workflows ./workflows \
  --token-file .nika/serve.token \
  --max-concurrent-jobs 16 --queue-capacity 2000 \
  --max-queued-per-owner 50 --max-concurrent-per-owner 2 \
  --job-retention 24h --shutdown-grace 5m
```

- A free worker goes to the owner with the fewest jobs running, in turn
  among equals. A user who submits fifty jobs does not delay the user who
  submits one. Jobs without an `owner` share one lane.
- `--max-queued-per-owner` refuses the owner's next job with
  `429 owner_queue_full` and a `Retry-After` header. Nothing is written:
  send the same request again, with the same `Idempotency-Key`.
- `--max-concurrent-per-owner` refuses nothing: the owner's other jobs wait
  while free workers serve other owners.
- `--queue-capacity` bounds all waiting jobs together (`503 queue_full`), and
  `--max-jobs` the jobs the store keeps (`507 job_capacity`). Every job
  operation rewrites the whole store, so prefer a retention window to a
  larger `--max-jobs`.
- `--shutdown-grace` is how long a stop waits for running and queued jobs
  (`30s` by default). Give your supervisor a longer kill timeout: with
  systemd, `TimeoutStopSec` of the grace plus 15 seconds.
- A scheduled run is never refused or held back by a per-owner limit.

Read what the resident holds at any time:

```sh
curl -sS -H "Authorization: Bearer $(cat .nika/serve.token)" \
  http://127.0.0.1:8787/v1/occupancy
```

The answer counts the jobs running and waiting, repeats the limits, and names
the busiest owners (at most 100). The owner is what your backend states: it
shares the workers fairly, it is not a security boundary between users.

## Managing schedules over HTTP

A resident whose `/health` lists `scheduleManage` lets an operator see,
remove and run the schedules without touching its files:

```sh
TOKEN="Authorization: Bearer $(cat .nika/serve.token)"
# Every schedule: the ones this door created, then the project's beats.
curl -s http://127.0.0.1:8787/v1/schedules -H "$TOKEN"
# Run one now, once. The cadence does not move.
curl -s -X POST http://127.0.0.1:8787/v1/schedules/nightly/trigger \
  -H "$TOKEN" -H 'Idempotency-Key: nightly-by-hand-1'
# Remove one, whichever revision is current.
curl -s -X DELETE http://127.0.0.1:8787/v1/schedules/nightly \
  -H "$TOKEN" -H 'If-Match: *'
```

A trigger admits an ordinary job: it waits in the same queue, counts
against the same limits, and shows in `GET /v1/jobs` with
`"origin":{"kind":"manual"}`. It runs under what the schedule declares,
including its `owner` and `labels`, and takes no body. Send the same
`Idempotency-Key` again and the answer is the same job, never a second
run.

Removing a schedule does not stop a job it already fired: cancel that job
by its id. To remove only if nobody changed the schedule since you read
it, send the `ETag` of that read in `If-Match` instead of `*`; a changed
schedule answers `412` and stays. The project's beats are declared in
`nika.yaml`: remove or change them there, and run one by hand with
`nika arm fire`. Over HTTP both gestures answer
`409 schedule_declared_by_project`.

## Runs on a route the catalog does not price

A workflow whose model the engine's catalog does not price (an API route
with no tariff row) cannot run under a USD ceiling: no amount can bound a
price nobody knows. By default every such job is refused, and the only way
through is one human decision per run (`--cost-review`, then
`POST /v2/cost-reviews`, a decision, and one job carrying that review).

`--cost-policy` replaces that per-run decision with one standing
authorization, made once at startup. Each rule names one model and the
bounds the engine enforces without a price:

```sh
nika serve --bind 127.0.0.1:8787 --workflows ./workflows --token-file .nika/serve.token \
  --run-cost-ceiling none \
  --cost-policy 'scaleway/deepseek-v4-flash-0731:requests=4,output-tokens=4096,timeout=120,runs=200/day'
```

| Setting | Bound |
|---|---|
| `requests=N` | Requests one run may send, schema re-asks and authored retries included (1 to 256) |
| `output-tokens=N` | Output tokens one request may ask (1 to 8192) |
| `timeout=SECONDS` | Seconds one request may wait (1 to 120) |
| `runs=N/PERIOD` | Runs per `minute`, `hour`, `day`, `week` or `<seconds>s`, counted in `.nika/standing-cost-runs.json` and kept across restarts |
| `request-bytes=N` | Optional: the largest request body (default and maximum 1 MiB) |
| `input-usd=`, `output-usd=` | Optional, together: your price per million tokens |

The four bounds are required, and the flag repeats, one rule per model. A
job on a named route then runs with no review when its run fits the rule.
A run that may send more requests or ask more output than the rule admits
is refused before anything is sent, as is a route no rule names and the run
past a full window.

Runs are counted per route in the project, not per rule. Serve a project
from one server and keep a route's window when you edit its rule: an
admission under a shorter window (a second server on the same project, or a
restart with an edited rule) forgets the runs a longer window still counted,
so the longer rule's count starts again from what is left.

A rule overrides no ceiling. Without a price, every ceiling the job is
under must be absent: the server's (`--run-cost-ceiling none`), the job's
own and the project's `ceiling:`. Scheduled occurrences always carry
`maxCostUsd`, so without a declared price they stay refused. With
`input-usd` and `output-usd` the engine computes the run's declared worst
case (every request at its size and output bounds, at your rates) and
admits the run only when that figure fits under each ceiling, so the
default ceiling can stay armed and a schedule can run on the route:

```sh
nika serve --bind 127.0.0.1:8787 --workflows ./workflows --token-file .nika/serve.token \
  --cost-policy 'scaleway/deepseek-v4-flash-0731:requests=4,output-tokens=4096,timeout=120,runs=200/day,request-bytes=32768,input-usd=0.15,output-usd=0.60'
```

A declared price is your estimate, never the provider's invoice: the job
records it as `host_declared`, and a call whose usage the engine cannot
qualify is still recorded with an unknown cost, never zero.

What stays as it was: the workflow shape (direct `infer:` steps with a
literal model and `max_tokens`, local `nika:read`, `nika:write`, `nika:jq`
and `nika:assert`, no agent, exec or nested workflow), one such run at a
time per project (a second job waits for the first), and the rule that a
request which left without a usable answer blocks every later run on an
unpriced route until you reconcile it with `nika trace cost`.

A job a rule admitted says so: the `settlement` of `GET /v1/jobs/{id}` and
of the job's terminal event carries `cost_admission` with the rule's
identity, the bounds, the price state and the ceiling evidence, and the
cost journal and the trace record the same choice. It rides inside the
settlement because a published SDK refuses a job key it does not know and
reads a settlement open: a client on the current SDK reads these jobs
unchanged, and finds the admission at `settlement.cost_admission`.
`/health` lists `costStandingPolicyV1`. Beside `--cost-review`,
`POST /v2/cost-reviews` answers `review_required: false` with the rule for
such a run, so a client that asks for a review before every job needs no
change. The cost journal of a server that holds a policy is archived beside
itself (`.nika/inference-cost-observations.<digest>.ndjson`) when it nears
its 1 MiB read bound and every run in it settled clear.

## Files here

- `nika-serve.service` — systemd unit, loopback, SIGTERM drain.
- `Caddyfile.nika-serve` — TLS + unbuffered SSE. Caddy terminates TLS;
  `nika doctor` never claims TLS from a proxy guess.

MCP servers a workflow invokes — the registry, the approve step, the
confinement: [mcp-servers.md](mcp-servers.md).
