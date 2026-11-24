// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The resident CLI vocabulary, shared without a dependency on the server.

use std::path::PathBuf;

/// `nika serve`'s explicit native authoring seat (the `nika compile` flag words; only the
/// reasoning effort falls back to the environment). Absent `--authoring-model`, nothing is read
/// and nothing changes.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct NativeAuthoringArgs {
    /// Seat native authoring on POST /v1/compile generation 2 with this direct provider model
    /// (`provider/name`); each caller still opts in (`cognition: "explicitProvider"`). Its key
    /// and endpoint are read from the environment now, never per request. Requires `--bind`.
    #[arg(
        long = "authoring-model",
        value_name = "PROVIDER/NAME",
        requires = "bind"
    )]
    pub model: Option<String>,
    /// Output tokens per call (positive); absent, the route's own completion bound. A caller
    /// may only narrow it.
    #[arg(long = "authoring-max-tokens", value_name = "N", requires = "model")]
    pub max_tokens: Option<u32>,
    /// Seconds per model invocation (positive); absent, the route's own wait. Resends consume
    /// the request grant.
    #[arg(long = "authoring-timeout", value_name = "SECONDS", requires = "model")]
    pub timeout: Option<u64>,
    /// Seconds per round (positive): the work stops and the request answers 408. Absent, no
    /// round deadline.
    #[arg(
        long = "authoring-deadline",
        value_name = "SECONDS",
        requires = "model"
    )]
    pub deadline: Option<u64>,
    /// Desired repair rounds, within the operator's explicit request grant. Absent, no count:
    /// repairs continue while they make progress.
    #[arg(long = "authoring-repairs", value_name = "N", requires = "model")]
    pub repairs: Option<u32>,
    /// A Foundry knowledge release root, admitted and pinned at start against a trusted identity;
    /// a flag carries none, so the seat is refused until one is wired.
    #[arg(long = "knowledge", value_name = "DIR", requires = "model")]
    pub knowledge: Option<PathBuf>,
    /// A corpus whose examples the knowledge door never recalls.
    #[arg(
        long = "knowledge-exclude",
        value_name = "CORPUS",
        requires = "knowledge"
    )]
    pub knowledge_exclude: Option<String>,
    /// Turn the knowledge off on the seat's own layer: nothing is pinned (beside --knowledge,
    /// the shared parser refuses the seat).
    #[arg(long = "no-knowledge", requires = "model")]
    pub no_knowledge: bool,
    /// The reasoning effort every seat call asks (low · high · max), sent only where the route
    /// qualifies it; the one word `NIKA_AUTHORING_REASONING` names when the flag is absent.
    #[arg(long = "authoring-reasoning", value_name = "LEVEL", requires = "model")]
    pub reasoning: Option<String>,
    /// A decision model (`typesafe/jev-1.13.0` or `provider/name`, the `nika compile` words) that
    /// judges every candidate in place of the author, on its own client; its key is read now.
    #[arg(long = "decision-model", value_name = "MODEL", requires = "model")]
    pub decision_model: Option<String>,
}

const SHUTDOWN_HELP: &str = "Shutdown (persistent mode): Ctrl-C/SIGINT and SIGTERM stop HTTP admissions \
and new scheduling, then drain running AND queued jobs for up to 30 seconds \
with four workers (--shutdown-grace and --max-concurrent-jobs set another \
grace and worker count). On grace expiry, running jobs become interrupted and \
jobs still queued remain queued. Restart with the same --state-root resumes queued \
jobs from their captured snapshots; an interrupted job stays interrupted unless \
--retry-interrupted starts it again or a POST /v1/jobs/{id}/retry request asks for it. \
SIGKILL skips cleanup: the next start interrupts ownerless running jobs (the same \
policy applies to them) and resumes queued jobs. A completed drain exits 0; grace expiry exits 1. The grace \
bounds execution draining, not filesystem cleanup or a stuck backend. \
Allow extra time beyond it before a supervisor forces SIGKILL.";

/// `nika serve` — the resident firer's args, plus the explicit HTTP pair.
#[allow(clippy::struct_excessive_bools)] // one bool per independent CLI switch
#[derive(Debug, Clone, Default, clap::Args)]
#[non_exhaustive]
#[command(after_long_help = SHUTDOWN_HELP)]
pub struct ServeArgs {
    /// Fire what is due once, then exit — the rehearsal.
    #[arg(long)]
    pub once: bool,
    /// Say what WOULD fire, run nothing.
    #[arg(long)]
    pub dry: bool,
    /// Inject the clock (RFC 3339 · D5) — the harness.
    #[arg(long, hide = true, value_name = "RFC3339")]
    pub now: Option<String>,
    /// Stop the loop at this instant (RFC 3339) — the harness.
    #[arg(long, hide = true, value_name = "RFC3339")]
    pub until: Option<String>,
    /// Bind an authenticated HTTP listener. Requires `--workflows` and `--token-file`.
    #[arg(long, value_name = "ADDR")]
    pub bind: Option<String>,
    /// The served registry: the listener lists, schedules and ADMITS BY NAME
    /// (`POST /v1/jobs {"workflow": "<name>"}`) only the `.nika` workflows
    /// under this directory, named from the project root. A remote world
    /// rides the snapshot `nika check <file> --json --sdk-snapshot` prints.
    /// Requires `--bind`.
    #[arg(long, value_name = "DIR")]
    pub workflows: Option<PathBuf>,
    /// Acknowledge a non-loopback `--bind`. Authentication is unchanged.
    /// TLS is a reverse proxy — this process does not terminate it.
    #[arg(long)]
    pub allow_remote: bool,
    /// Owner-only Bearer file (32–512 visible ASCII bytes, mode 0600). Never argv.
    /// Mint: umask 077 && mkdir -p .nika && openssl rand -hex 24 > .nika/serve.token
    /// && chmod 600 .nika/serve.token
    #[arg(long, value_name = "FILE")]
    pub token_file: Option<PathBuf>,
    /// Durable job-state root. Defaults to `<cwd>/.nika/serve`.
    #[arg(long, value_name = "DIR")]
    pub state_root: Option<PathBuf>,
    /// Forget a succeeded job once it has been settled this long (`36h`, `7d`; a whole number
    /// and s, m, h or d; at least one minute). Absent, every job is kept and a full store
    /// refuses every admission. A forgotten job's Idempotency-Key is forgotten with it, so
    /// keep this longer than your callers retry. Trace journals are not removed. Persistent
    /// mode only.
    #[arg(long, value_name = "DURATION", conflicts_with_all = ["once", "dry"])]
    pub job_retention: Option<String>,
    /// The same for failed, cancelled and interrupted jobs. Absent, they follow
    /// `--job-retention`.
    #[arg(long, value_name = "DURATION", conflicts_with_all = ["once", "dry"])]
    pub job_retention_unsuccessful: Option<String>,
    /// Run at most this many jobs at once (default 4, at most 256). Persistent mode only,
    /// like every limit below.
    #[arg(long, value_name = "N", conflicts_with_all = ["once", "dry"])]
    pub max_concurrent_jobs: Option<usize>,
    /// Let at most this many jobs wait for a worker, all owners together (default 64). Past it
    /// an admission answers 503 `queue_full`.
    #[arg(long, value_name = "N", conflicts_with_all = ["once", "dry"])]
    pub queue_capacity: Option<usize>,
    /// Keep at most this many jobs in the store (default 10000). At it every admission
    /// answers 507 until `--job-retention` forgets some. Every job operation rewrites the
    /// whole store, so a larger store makes each one slower.
    #[arg(long, value_name = "N", conflicts_with_all = ["once", "dry"])]
    pub max_jobs: Option<usize>,
    /// Let one owner have at most this many jobs waiting. Past it that owner's admission
    /// answers 429 `owner_queue_full` with Retry-After; other owners are still admitted.
    /// Jobs that state no owner count as one owner. Absent, only `--queue-capacity` bounds
    /// an owner.
    #[arg(long, value_name = "N", conflicts_with_all = ["once", "dry"])]
    pub max_queued_per_owner: Option<usize>,
    /// Run at most this many jobs of one owner at once; its other jobs wait while free
    /// workers serve other owners. Absent, only `--max-concurrent-jobs` bounds an owner.
    #[arg(long, value_name = "N", conflicts_with_all = ["once", "dry"])]
    pub max_concurrent_per_owner: Option<usize>,
    /// At a stop, wait this long for running and queued jobs to end (`30s` by default; a
    /// whole number and s, m, h or d; at least one second). Past it running jobs settle
    /// interrupted. Give the supervisor a longer kill timeout than this.
    #[arg(long, value_name = "DURATION", conflicts_with_all = ["once", "dry"])]
    pub shutdown_grace: Option<String>,
    /// What becomes of a job whose run was lost to a crash, a kill or a stop past its grace:
    /// `off` (default) leaves it interrupted; `restart` runs its captured workflow again from
    /// the first task, as a new attempt of the same job, under what is left of its cost
    /// ceiling; `resume` serves the tasks the lost attempt's journal completed and runs the
    /// rest, once the journal is judged as `nika run --resume` judges one. A workflow that can repeat an effect outside the engine is started again only
    /// if its job stated `retry` when it was submitted. A job may state its own mode.
    #[arg(long, value_name = "MODE", conflicts_with_all = ["once", "dry"])]
    pub retry_interrupted: Option<String>,
    /// The most attempts one job gets, counting the first (default 3, at most 16). A job may
    /// state a lower bound, never a higher one.
    #[arg(long, value_name = "N", conflicts_with_all = ["once", "dry"])]
    pub retry_max_attempts: Option<usize>,
    /// Keep at most N model calls of `infer` and `agent` tasks in flight on one provider,
    /// across every job (`openai=16`; repeat for each provider). A call past it waits its
    /// turn, inside its task's `timeout:`; retries are unchanged. Calls through an agent
    /// harness and the image and speech tools are not counted. Absent, each job's own
    /// fan-out is the only bound on a provider's calls.
    #[arg(long, value_name = "PROVIDER=N", conflicts_with_all = ["once", "dry"])]
    pub provider_concurrency: Vec<String>,
    #[command(flatten)]
    pub authoring: NativeAuthoringArgs,
    /// Explicit model invocation and physical-request ceiling per authoring round; absent, no
    /// count bounds the round's requests (its repair preference and deadline still apply).
    #[arg(long, value_name = "N", requires = "model")]
    pub authoring_max_calls: Option<u32>,
    /// Seat the cost-review door (POST /v1 and /v2/cost-reviews · health `costReviewV1`/`V2`).
    #[arg(long, requires = "bind")]
    pub cost_review: bool,
    /// Per-run spend ceiling of manual jobs in USD (default 1); `none` disarms it explicitly.
    #[arg(long, value_name = "USD|none", requires = "bind")]
    pub run_cost_ceiling: Option<String>,
    /// Standing cost rule for one API route the catalog does not price (repeatable · health
    /// `costStandingPolicyV1`): a Run inside it needs no per-run cost review.
    /// `PROVIDER/MODEL:requests=N,output-tokens=N,timeout=SECONDS,runs=N/PERIOD`, PERIOD being
    /// minute, hour, day, week or `<seconds>s`; optionally `request-bytes=N`, and
    /// `input-usd=USD,output-usd=USD` (your price per million tokens, an estimate). Without a
    /// declared price no USD ceiling can bound the Run: every ceiling it is under must be
    /// absent (`--run-cost-ceiling none`). With one, its declared worst case must fit under
    /// them. A rule overrides no ceiling.
    #[arg(long = "cost-policy", value_name = "RULE", requires = "bind")]
    pub cost_policy: Vec<String>,
    /// Serve the project's native Session under /v1/sessions (health `sessionHost`): the same
    /// Session bare `nika` opens, whose runs are this resident's jobs. A Session writes the
    /// project's workflows only on a human's consent.
    #[arg(long, requires = "bind")]
    pub sessions: bool,
}
