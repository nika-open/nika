// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `nika explain NIKA-XXXX` — teach one error code (spec §2), or the
//! hint identity `nika check` printed in the same `[brackets]` slot.
//!
//! Two registries, ONE voice: the numeric crate registry
//! (`nika-error::codes` · `NIKA-440`) AND the spec's conformance codes
//! (`NIKA-DAG-002` · the canon's `error_codes` table) — every code the
//! checker can emit gets an answer here. Retired codes (`NIKA-DAG-003` ·
//! `NIKA-PARSE-016`) teach their retirement and point at the successor.
//! Hint kinds (`jq-as-map` · `native-first/006`) are the other occupant
//! of that slot (#1038) — `nika-check::hint_help` is the teaching, same
//! text the MCP tool returns. Never invents: a token in no registry is
//! a finding (`exit 2`), not a guess.

use nika_error::codes::{code_help, lookup};

use crate::display::theme::Theme;
use crate::output::VerbOutput;
use crate::probe::{seat_escape_tail, with_seat_tail_from};

/// The doc-site home for the error-code registry — the ONE https target the
/// explain surface names. Printed scheme-less (the established prose form);
/// the OSC-8 wrapper carries the scheme.
const DOCS_ERRORS_URL: &str = "https://docs.nika.sh/errors";
const DOCS_ERRORS_TEXT: &str = "docs.nika.sh/errors";

/// The `nika explain <code>` verb. Accepts `NIKA-440`, `NIKA-DAG-002`,
/// or the bare forms (`440` · `DAG-002`). On a TTY the doc-site
/// reference rides an OSC-8 hyperlink; a piped explain keeps its bytes.
/// The door the teaching is worded for (ADR-124 · one ladder, two
/// doors): the CLI names `nika check --fix`; the oracle names the fix
/// an agent without a shell can reach — `nika_check` with `fix: true`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Door {
    /// The operator's terminal.
    #[default]
    Cli,
    /// The MCP oracle (read-only · source in, verdict out).
    Oracle,
}

impl Door {
    fn reword(self, hint: &str) -> String {
        match self {
            Self::Cli => hint.to_owned(),
            Self::Oracle => hint.replace("`nika check --fix`", "`nika_check` with `fix: true`"),
        }
    }
}

/// The CLI door of [`run_for`].
#[must_use]
pub fn run(wire: &str, theme: Theme) -> VerbOutput {
    run_for(wire, theme, Door::Cli)
}

/// The four-rung ladder — a hint kind · the registry · the spec rows ·
/// the namespaces — worded for `door`. Every rung answers from the
/// tables embedded in this binary; the two auth-class pages alone also
/// ask this machine for a ready seat ([`seat_escape_tail`]).
#[must_use]
pub fn run_for(wire: &str, theme: Theme, door: Door) -> VerbOutput {
    run_with_seat_tail(wire, theme, door, seat_escape_tail)
}

/// [`run_for`] over an injected source of the seat escape line. The real
/// source is the access census, which launches the login command of each
/// installed agentic CLI: it is asked at most once, and only by a code
/// whose page prints the line (`with_seat_tail_from` owns that gate).
/// Explaining any other code launches nothing and reads no key. Tests
/// inject a counting source.
fn run_with_seat_tail(
    wire: &str,
    theme: Theme,
    door: Door,
    seat_tail: impl FnOnce() -> Option<String>,
) -> VerbOutput {
    // The seam (`Theme::link` → `format::osc8`): text unchanged, escapes
    // only when the links capability resolved on.
    let docs = theme.link(DOCS_ERRORS_URL, DOCS_ERRORS_TEXT);
    // A HINT row prints `kind` (or a numbered `code`) in the same
    // bracketed slot as `NIKA-PARSE-019`. Resolve that token before
    // wrapping `NIKA-` — `jq-as-map` is not `NIKA-jq-as-map` (#1038).
    if let Some(help) = nika_check::hint_help(wire) {
        return VerbOutput::ok(format!("{wire} · hint\n\n  {help}\n"));
    }
    // The resident's wire codes (`nika serve` · #1441 · ADR-131): the same
    // voice as the engine's, from the same table the MCP tool reads.
    if let Some(text) = nika_error::codes::resident_help(wire) {
        return VerbOutput::ok(text);
    }
    let normalized = if wire.starts_with("NIKA-") {
        wire.to_owned()
    } else {
        format!("NIKA-{wire}")
    };
    let Some(code) = lookup(&normalized) else {
        // Not a numeric registry code — the spec conformance codes
        // (NIKA-DAG-002 …) live in the embedded canon's error_codes
        // table. Same binary, same single source of truth.
        if let Some(text) = canon_row(&normalized, door) {
            return VerbOutput::ok(with_seat_tail_from(&normalized, seat_tail, text));
        }
        // Retired conformance codes are ANSWERED, not 404'd: the hole in
        // the registry is deliberate (spec 05 · never reuse) and the
        // teaching survives — what the class became, and where it went.
        if let Some(text) = retired_row(&normalized) {
            return VerbOutput::ok(text);
        }
        // Runtime namespaces (per-builtin `NIKA-BUILTIN-<NAME>-<NNN>` ·
        // per-provider `NIKA-PROVIDER-<NNN>`) have no per-code registry
        // row — the shared teaching lives in `nika-error::codes` so the
        // MCP explain tool answers with the SAME text (one voice).
        if let Some(text) = nika_error::codes::namespace_help(&normalized, &docs) {
            return VerbOutput::ok(text);
        }
        // The MCP pin refusals the engine mints before the canon lists them (NIK-254): a run
        // that refuses with one prints `fix: nika explain <code>`.
        if let Some(text) = mcp_pin_help(&normalized) {
            return VerbOutput::ok(text);
        }
        // The last rung before the 404: a code nothing served is often ONE
        // edit from a real one (`NIKA-PARSE-01` · `NIKA-DGA-003` — V9 wave 3
        // row 11, p01: « no way to LIST or SEARCH codes … if a sceptic
        // mistyped a code, they'd be stuck »). The metric is the engine's
        // ONE metric (`nika_types::suggest` · Damerau-Levenshtein, rustc's
        // threshold), over the canon's OWN registry — never a second
        // spelling of « close », and silence when nothing is near enough.
        let rows = nika_pack::error_codes();
        let near = nika_types::suggest::did_you_mean(&normalized, rows.iter().map(|r| r.code))
            .map_or_else(String::new, |code| format!("\n\n  did you mean `{code}`?"));
        return VerbOutput::file(format!(
            "unknown code `{wire}` — the registry knows NIKA-001..NIKA-9999 \
             (allocated ranges), the spec conformance codes \
             (NIKA-DAG-* · NIKA-VAR-* · …), per-builtin NIKA-BUILTIN-<NAME>-NNN \
             and per-provider NIKA-PROVIDER-NNN codes, and the hint kinds \
             `nika check` prints in [brackets]; see {docs}{near}"
        ));
    };
    // The category/severity labels are the OWNING crate's canonical
    // kebab-case (`Category::as_str`), not a `Debug` derive an enum rename
    // could silently change — one source of truth, compile-forced complete.
    let text = format!(
        "{code} · {category} · {severity} · {slug}\n\n  {help}\n",
        category = code.category.as_str(),
        severity = code.severity.as_str(),
        slug = code.slug,
        help = code_help(code),
    );
    VerbOutput::ok(with_seat_tail_from(&normalized, seat_tail, text))
}

/// Teach a spec conformance code from the embedded canon's registry —
/// through THE one typed parser ([`nika_pack::error_codes`] · its
/// anchoring, malformed-row tolerance and escape-free invariant are
/// pinned at the nika-pack seam, not re-rolled here).
fn canon_row(code: &str, door: Door) -> Option<String> {
    let row = nika_pack::error_codes()
        .into_iter()
        .find(|r| r.code == code)?;
    // The contract lesson, when the code earned one (one voice: the MCP
    // explain appends the same text — `nika_error::codes::spec_contract_help`).
    let lesson = match code {
        "NIKA-VALUES-001" => Some(nika_schema::error::DeadForm::Vars.field_teaching()),
        "NIKA-VALUES-002" => Some(nika_schema::error::DeadForm::Env.field_teaching()),
        _ => nika_error::codes::spec_contract_help(code).map(str::to_owned),
    }
    .map(|lesson| format!("\n{}", door.reword(&lesson)))
    .unwrap_or_default();
    let fix = cli_fix_hint(code)
        .map(|h| format!("  fix: {}\n\n", door.reword(h)))
        .unwrap_or_default();
    Some(format!(
        "{code} · {category} · transient: {transient}\n\n  {failure}\n{lesson}\n{fix}\
         full docs: https://nika.sh/language/errors/{code} — {closer}\n",
        category = row.category,
        transient = row.transient,
        failure = row.failure,
        closer = closer_line(code),
    ))
}

/// The canon row's closing claim — TRUE per code class (V7-2 · wave-3:
/// four personas read « `nika check` catches this before a run ever
/// starts » under a refusal that check CANNOT catch — a computed path
/// is the run's to judge — and Marta « stopped running check at all »).
/// A teaching surface must never promise more than the judge checked.
fn closer_line(code: &str) -> &'static str {
    if let Some(closer) = run_verdict_closer(code) {
        return closer;
    }
    match code {
        // The boundary refusals: check judges the LITERAL shape (a
        // written path · a `const:`-resolved arg); a computed path (a
        // glob result · an interpolated binding · an upstream output)
        // is judged at RUN — measured: a dir-only grant checked
        // `✔ PERMITS`, then every per-item read died SEC-004.
        "NIKA-SEC-004" => {
            "`nika check` catches the LITERAL shape before a run; a \
             computed path (a glob result · an interpolated binding) or a \
             redirect hop is judged at RUN — a green PERMITS is not its \
             promise."
        }
        // #1582: check refuses a literal floor target; the hop a redirect
        // makes, and a name that RESOLVES into the floor, exist only at run.
        "NIKA-SEC-005" => {
            "`nika check` catches a literal loopback, private or metadata URL \
             before a run; a redirect hop and a host that resolves into that \
             space are judged at RUN."
        }
        // The wire refusals: check resolves the MODEL ID in this binary
        // but never dials the server — measured (gauntlet wave-4,
        // founder-fr): a green check printed « local servers not probed »
        // itself, then the run died INFER-001 on a mute endpoint. The
        // closer must not promise the dial it never made.
        "NIKA-INFER-001" | "NIKA-INFER-003" => {
            "`nika check` resolves the model in this binary; the wire \
             itself — a live server, a valid key, a priced usage block — \
             is the RUN's verdict (`nika doctor --ping` dials ahead)."
        }
        // The exec floor (#605): check catches the literal argv (the SAME
        // predicate the run judges) — a `${{ }}` island is the RUN's verdict.
        // #1592: the OS sandbox is named as `nika-runtime-laws`
        // `select_command_sandbox` picks it (the seatbelt and landlock crates).
        "NIKA-SEC-001" => {
            "`nika check` catches a literal argv before a run (the same \
             floor predicate the run judges with); a templated command — \
             a `${{ }}` island — is judged at RUN. A confined child the \
             OS sandbox denied is also this code: `exec` runs inside the \
             sandbox built from `permits:` — Seatbelt (`/usr/bin/sandbox-exec`) \
             on macOS, bubblewrap (`/usr/bin/bwrap`) on Linux, the backend \
             `nika doctor` names. A non-zero exit whose stderr reports a denial \
             is classified as the sandbox's refusal, but the engine reads only \
             that stderr text: a program's own permission error (a file its OS \
             permissions refuse, inside the grant) reads the same. The refusal \
             names the path when stderr carries it. Check the relevant \
             `permits.fs.read` / `permits.fs.write` and OS access, the file's \
             own permissions included; an unknown operation stays unknown (a \
             path on stderr is not proof a grant is missing)."
        }
        // #1396: check's own EXEC row says « a templated argv is the
        // RUN's verdict », and an exit status can never be a check-time
        // finding — the blanket closer contradicted the card it explained.
        "NIKA-EXEC-001" => {
            "`nika check` refuses a LITERAL argv the exec floor would refuse; a \
             templated program name and a non-zero exit status are the RUN's \
             verdict — an exit status can never be a check-time finding."
        }
        _ => "`nika check` catches this before a run ever starts.",
    }
}

/// The closer of a class check cannot raise, or raises only in part (NIK-402): the run's
/// verdict, never « `nika check` catches this ».
fn run_verdict_closer(code: &str) -> Option<&'static str> {
    let closer = match code {
        // NIK-402: a schema verdict exists only once an answer does. Check reads the
        // schema's shape; the answer is the run's, after the engine's own repairs.
        "NIKA-INFER-002" => {
            "a RUN verdict: `nika check` reads the `schema:` itself, never an answer, so it \
             cannot catch this. The engine repairs a miss before it fails (a re-ask up to \
             the repair budget, or one repair prompt in the same session for an `agent:` \
             on a seat), and `error.details` keep the rejected answer (`partial_output`) \
             and every violation; \
             `on_codes: [NIKA-INFER-002]` in a `retry:` block asks for a fresh attempt."
        }
        // thinking.rs: check names a catalog-known reasoning seat under a tiny cap.
        "NIKA-INFER-004" => {
            "`nika check` catches a `max_tokens` cap too small for a catalog-known \
             reasoning seat before a run; a blank answer under any other cap is the RUN's \
             verdict."
        }
        // schema_typing.rs: check refuses a `for_each` over a declared non-array.
        "NIKA-VAR-006" => {
            "`nika check` catches a `for_each` over a constant or an input declared as a \
             non-array before a run; every other type error at evaluation is the RUN's \
             verdict."
        }
        // The classes only a run raises (NIK-402): a budget, a timeout, a cancellation,
        // a spawn, a tool call, a decision, a value at evaluation.
        "NIKA-INFER-005" | "NIKA-EXEC-002" | "NIKA-AGENT-001" | "NIKA-AGENT-002"
        | "NIKA-AGENT-005" | "NIKA-AGENT-006" | "NIKA-AGENT-007" | "NIKA-SEC-002"
        | "NIKA-SEC-003" | "NIKA-SEC-010" | "NIKA-SEC-011" | "NIKA-TIMEOUT-001"
        | "NIKA-CANCEL-001" | "NIKA-MCP-001" | "NIKA-MCP-002" | "NIKA-DECIDE-001"
        | "NIKA-DECIDE-002" | "NIKA-TYPE-101" | "NIKA-VAR-002" | "NIKA-VAR-004"
        | "NIKA-VAR-009" => {
            "a RUN verdict: this engine raises it while the workflow runs, and a clean \
             `nika check` does not rule it out."
        }
        // Rows the spec reserves or defines that this engine does not raise yet.
        "NIKA-PARSE-004" | "NIKA-TYPE-005" | "NIKA-VAR-007" | "NIKA-PORT-001" | "NIKA-PORT-002"
        | "NIKA-ASSERT-001" | "NIKA-LOCK-001" => {
            "this engine raises it neither at check nor at run yet: the spec reserves or \
             defines the row ahead of the engine."
        }
        _ => return None,
    };
    Some(closer)
}

/// The MCP pin family `nika-mcp` mints (`PinError` · `pin.rs`) and the canon table does not
/// carry yet (NIK-254): what failed, then the remedy the refusal itself prints. 003 to 005 wear
/// the category the spec registry reserves for them; 006 has no registry row yet and wears the
/// security category of the pin refusals beside it. The rung goes once the canon carries these
/// rows: its tripwire test fails then.
fn mcp_pin_help(code: &str) -> Option<String> {
    let (category, meaning, fix) = match code {
        "NIKA-MCP-003" => (
            "security_error",
            "the tools a pinned MCP server serves drifted from the definitions approved in \
             `.nika/mcp_pins.json` (a changed description or input schema, a tool added or \
             removed): no tool from that server reaches the run until a human re-approves it",
            "if the change is expected, review it and re-pin deliberately: `nika mcp approve \
             <server>`; if not, the server or its supply chain changed under you: investigate \
             it first, or drop the server from `.nika/mcp_servers.json`",
        ),
        "NIKA-MCP-004" => (
            "security_error",
            "`.nika/mcp_pins.json` exists but cannot be trusted (it does not parse, its format \
             is unknown, or an entry does not match its pin): it is never read as empty and was \
             not rewritten, since a corrupt or hand-edited lockfile is itself a tamper signal",
            "restore the file from version control; to re-enroll instead, delete it \
             deliberately, then re-approve every server: `nika mcp approve <server>`",
        ),
        "NIKA-MCP-005" => (
            "process_error",
            "the OS sandbox could not confine the MCP server's process (the mechanism is \
             missing, or the project path cannot be confined): the server was not started, and \
             running it unconfined is never the fallback",
            "check that the platform sandbox is present (`sandbox-exec` on macOS, `bwrap` on \
             Linux, as `nika doctor` names it) and that the project path can be confined, then \
             retry",
        ),
        "NIKA-MCP-006" => (
            "security_error",
            "the run named an MCP tool that is not approved: its server is declared in \
             `.nika/mcp_servers.json` but was never approved, or the tool is outside the set \
             it was approved with; the server was not started",
            "review the server's tools and approve them: `nika mcp approve <server>` (again, \
             when the server now serves the tool); `nika check` names each unapproved call in \
             its `[mcp-pins]` hint",
        ),
        _ => return None,
    };
    Some(format!(
        "{code} · {category} · an MCP pin refusal\n\n  {meaning}.\n  fix: {fix}.\n"
    ))
}

/// The retirement teaching for a conformance code the canon table no
/// longer carries (spec 05 · retired codes are never reused — the
/// allocation hole is deliberate). States what the class BECAME so an
/// old trace, doc or memory that names the code still gets an answer.
fn retired_row(code: &str) -> Option<String> {
    // (teaching, the LIVE successor code — its page is the one that
    // exists; a retired per-code URL is a 404, measured 2026-08-01:
    // the site's error pages project the CURRENT canon table, and a
    // retired code is exactly the row it no longer carries.)
    let (teaching, successor) = match code {
        "NIKA-DAG-003" => (
            "« a `tasks.X` reference with no declared edge » became \
             INEXPRESSIBLE in W2 « the flow »: the `with:` binding IS the \
             edge (derived, never restated), and a reference outside the \
             boundary is NIKA-VAR-021 (hoist it into `with:` — \
             `nika check --fix` applies it)",
            "NIKA-VAR-021",
        ),
        "NIKA-PARSE-016" => (
            "the jq-binding-contains-template class folded into \
             NIKA-VAR-005 at the deep-conformance registry remap",
            "NIKA-VAR-005",
        ),
        _ => return None,
    };
    Some(format!(
        "{code} · retired — never reused\n\n  {teaching}\n\n\
         full docs: https://nika.sh/language/errors/{successor} — the successor \
         code carries the live page; a retired code's own page is gone \
         with its registry row.\n"
    ))
}

/// The ENGINE-side actionable fix for a spec code, when this binary
/// ships one (the canon row states the FAILURE — per-CLI affordances
/// live here, never in the SSOT).
fn cli_fix_hint(code: &str) -> Option<&'static str> {
    match code {
        // R4: the credential refusal names the seat escape hatch (the env
        // ladder alone sent seated operators to a vendor signup).
        "NIKA-INFER-001" => Some(
            "set the key the witness names (custody is the process env: \
             `export <VAR>=…`), or run through a seat present on this machine: \
             `nika run <file> --access claude-code` — any agentic CLI you're \
             signed into serves (its login is judged when the run starts · \
             `nika doctor` lists them)",
        ),
        // The high-traffic conformance codes teach the one obvious YAML
        // edit concretely (#145 P1 — the fix-form lives here, never in
        // the canon row).
        "NIKA-DAG-001" => Some(
            "break the loop — one task in the cycle must drop the `with:` \
             binding or `after:` entry that closes it (a task can never \
             wait on itself, directly or through a chain)",
        ),
        "NIKA-DAG-002" => Some(
            "the `with:` binding or `after:` entry names a task that does \
             not exist — match it to a declared task key (check for a \
             typo first)",
        ),
        "NIKA-PARSE-024" => Some(
            "`depends_on` is dead since W2 — a data read becomes a `with:` \
             binding (the binding IS the edge · the body reads \
             `${{ with.<name> }}`), a pure ordering becomes `after: \
             {<task>: success}` (or `terminal` for the always-pattern); \
             `nika check --fix` reads a `depends_on:` whose entries are all \
             bare task ids (`[a, b]`, `[\"a\"]` or a `-` list — an empty \
             list declares no edge, so the dead line just drops) — a \
             scalar, a map entry or any other string is yours to rewrite, \
             and the finding says which shape it saw. The readable shape is \
             not a guarantee: `--fix` still stops the whole file when a \
             producer may SKIP (`when:` · `for_each:` · `on_error.skip`), \
             when a dep is backed only by a status read, or when a hoist \
             would leave `on_error:` armor — each stop names its case",
        ),
        "NIKA-DAG-005" => Some(
            "the `after:` predicate set is closed — pick one of `success` · \
             `failure` · `skipped` · `terminal` (`nika check --fix` respells \
             the dead R5 spellings)",
        ),
        "NIKA-DAG-006" => Some(
            "the task is statically dead — an incoming edge's pass-set \
             excludes every state its producer can reach (e.g. `after: \
             {x: skipped}` on a task that can never skip), or the `when:` \
             gate is false under every reachable upstream combination; \
             loosen the predicate, give the producer the missing route, \
             or delete the dead task",
        ),
        "NIKA-DAG-007" => Some(
            "the status vocabulary is closed — compare against `success` · \
             `failure` · `skipped` · `cancelled` (the finding's \
             did-you-mean names the nearest one)",
        ),
        "NIKA-VAR-021" => Some(
            "hoist the reference into `with:` — the binding declares the \
             edge, the body reads `${{ with.<name> }}` (`nika check --fix` \
             applies it)",
        ),
        "NIKA-PARSE-002" => Some(
            "every workflow starts with `nika: <kebab-id>` and a non-empty \
             `tasks:` map (keyed by task id); description is a `#` comment \
             above `nika:`",
        ),
        // #1608: the same causes the refusal names on the pointed line (`yaml_error.rs`).
        "NIKA-PARSE-001" => Some(
            "the YAML itself is broken — the usual causes, on the pointed line: a \
             `: ` inside a plain value (`prompt: Note: here`): quote the whole value \
             (`prompt: \"Note: here\"`), write it as a block scalar after `prompt: |`, \
             or, when the second key is a key of its own, put it on its own line, \
             indented under the first; an unquoted `${{ … }}` inside a flow mapping \
             `{ … }`: quote the expression (`{ prompt: \"${{ inputs.t }}\" }`); a \
             `{` or `[` the file never closes: close it. Otherwise look for a stray \
             tab (YAML forbids tabs), unbalanced quotes or a key missing its `:`; if \
             line 1 is blamed, a copier may have de-commented the \
             `# yaml-language-server:` modeline",
        ),
        "NIKA-PARSE-005" => Some(
            "the field is not in the closed envelope — the finding lists the \
             fields valid at that column (`the fields here: …`) and its \
             did-you-mean names a near spelling. A key from another dialect \
             lives inside a verb: `prompt:` → `infer:` · `http:` → `invoke:` \
             with `tool: nika:fetch` · `command:` / `script:` → `exec:` · `tool:` → \
             `invoke:`. Custom prose is a `#` comment above `nika:`",
        ),
        "NIKA-PARSE-019" => Some(
            "the field's YAML SHAPE is wrong (a string where a list goes, a \
             list where a map goes) — `tasks:` is a MAP keyed by task id, \
             and the finding names the field whose shape to fix",
        ),
        "NIKA-VAR-006" => Some(
            "the expression mixes types — `when:` must be boolean-shaped, \
             `for_each:` must reference an ARRAY (a `.output` of a `schema:` \
             task typed `{ array: … }`, or a literal list), and comparisons \
             need both sides the same type",
        ),
        _ => reference_fix_hint(code)
            .or_else(|| decide_fix_hint(code))
            .or_else(|| run_decl_fix_hint(code))
            .or_else(|| security_fix_hint(code)),
    }
}

/// The unresolved-reference hint — split out of [`cli_fix_hint`] at the
/// fn-length wall.
fn reference_fix_hint(code: &str) -> Option<&'static str> {
    match code {
        // F4: the unresolved-vars class is fixable from the CLI. #1599: a
        // task's result is the other common miss, and `config:` is a dead
        // envelope key (PARSE-005), never a root to teach.
        "NIKA-VAR-001" => Some(
            "an unbound `inputs:` entry is supplied on the CLI — `nika run <file> \
             --var <key>=<value>` (repeatable) — or given a `default:` in its \
             `inputs:` declaration. A task's result is `tasks.<id>.output`, never \
             a bare `<id>.output`: bind it in `with:` (`with: { page: \
             \"${{ tasks.fetch.output }}\" }`, quoted inside a flow mapping) and \
             read `${{ with.page }}` in the body. Other roots have other fixes: \
             `const:` resolves only from its own declared block, and `item` / \
             `index` exist only inside a `for_each` task",
        ),
        _ => None,
    }
}

/// The `decide` bundle hints — split at the fn-length wall.
fn decide_fix_hint(code: &str) -> Option<&'static str> {
    match code {
        "NIKA-DECIDE-001" => Some(
            "the bundle breaks its own laws — weights/thresholds are INTEGER \
             basis-points (8735 = 87.35% · never a float), rules read only \
             evidence_schema keys, identity keys never feed technical \
             dimensions, transforms are total on min..max, and the fixtures \
             must include a `contradictory` case and respect every declared \
             monotonicity (the reference evaluator refuses identically)",
        ),
        "NIKA-DECIDE-002" => Some(
            "the snapshot must satisfy the bundle's evidence_schema — every \
             item's key declared, its value fitting the declared spec-09 \
             type, its source in the authorized list, its integrity at or \
             above the declared floor; a MISSING required key is not an \
             error (the evaluation defers — abstention is a safety property)",
        ),
        _ => None,
    }
}

/// The `run:` declaration hints (F-P3 · NEP-0010 · the dedicated
/// mints) — split out of [`cli_fix_hint`] at the fn-length wall.
fn run_decl_fix_hint(code: &str) -> Option<&'static str> {
    match code {
        "NIKA-PARSE-026" => Some(
            "`entropy: ambient` declares live entropy while `clock: virtual` \
             demands a simulated clock — drop `clock: virtual`, or name the \
             stream (`entropy: none` or `{ seeded: <u64> }`)",
        ),
        "NIKA-PARSE-027" => Some(
            "`entropy: none | seeded` forces byte-identical journals; a wall \
             clock would leak real durations into them — drop `clock: system` \
             (the virtual clock is implied) or declare `clock: virtual`",
        ),
        "NIKA-PARSE-028" => Some(
            "`entropy: none` is a strict no-entropy promise but the body \
             consumes a structural source (a live retry jitter · `nika:uuid`) \
             — set `jitter: false`, drop the uuid call, or name the stream \
             with `entropy: { seeded: <u64> }`",
        ),
        _ => None,
    }
}

/// The security-boundary hints — split out of [`cli_fix_hint`] at the
/// fn-length wall.
fn security_fix_hint(code: &str) -> Option<&'static str> {
    match code {
        // The finding's own fix names only the gate. A missing leg is safe
        // by construction, while a gate is safe only while someone answers
        // it, so the page teaches the leg first. Both gate placements below
        // were checked clean on 0.124.5: after the reads, only the effect
        // past the gate tests the answer; upstream of every entry, a fetch
        // past the gate that ignores the answer fires NIKA-SEC-014.
        "NIKA-SEC-009" => Some(
            "first drop a leg the workflow does not need: the finding names \
             each leg and the grant that arms it (private read · \
             `permits.fs.read`; untrusted ingress · `nika:fetch` or an \
             `mcp:` server in `permits.tools`, or `exec`; external egress · \
             `permits.net.http`, an escaping `fs.write` glob or `exec`, so an \
             exec grant arms two legs), and a boundary missing a leg is safe \
             by construction. When all three are needed, add a blocking \
             `nika:prompt` (no `default:`, granted in `permits.tools`) that \
             every path to the egress task crosses, data edges included. \
             Placed after the reads and fetches, its question can show the \
             content that would leave; placed upstream of every entry task \
             the finding names, it is asked before the untrusted content \
             exists, so the person consents to the run, not to the payload. \
             Then bind its answer with `with:` into each effect that runs \
             after it, a fetch included, and gate that effect with `when:`: \
             NIKA-SEC-014 refuses an effect it can prove runs on a « no » \
             to a confirm gate. Only a `mode: confirm` gate counts by its \
             position alone. A `mode: choice` or `mode: input` gate answers \
             with text, not a yes: it counts only before an effect whose own \
             `when:` tests that answer (`when: ${{ with.pick == 'send' }}` on \
             a `with:` binding of its output, or on the output of a `nika:jq` \
             stage whose program reads it), and an effect that only waits on \
             it runs whatever the answer, so it is refused as if no gate stood \
             there. An input gate also counts before a `nika:` tool call that \
             sends only the typed answer: what it sends is what the person \
             typed. A gate of any mode, a confirm included, with an \
             `on_error:` or a `for_each:` counts before no effect: a recovered \
             value or a list of answers is not a person's answer",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::exit;

    /// The sober register (links off) — the byte-frozen baseline every
    /// machine surface reads, on a machine with no seat: the pages are
    /// read without the census, which would launch the agentic CLIs
    /// installed on the machine running the tests.
    fn run(wire: &str) -> VerbOutput {
        run_with_seat_tail(wire, Theme::new(false, false, false), Door::Cli, || None)
    }

    /// One explain over a counting source of the seat line: the answer,
    /// and how many times the source was asked.
    fn run_counting(wire: &str, door: Door, seat_line: Option<&str>) -> (VerbOutput, usize) {
        let asked = std::cell::Cell::new(0_usize);
        let out = run_with_seat_tail(wire, Theme::new(false, false, false), door, || {
            asked.set(asked.get() + 1);
            seat_line.map(str::to_owned)
        });
        (out, asked.get())
    }

    /// The seat line comes from the access census, which launches the
    /// login command of each installed agentic CLI. Only a page that
    /// prints the line may ask for it. Derived over both registries
    /// (never a hand-kept list): the codes that ask are the two
    /// auth-class ones, once each, and no other page shows the line.
    #[test]
    fn only_the_two_auth_class_codes_ask_for_the_seat_line() {
        const SEAT: &str = "SEAT-LINE";
        let canon = nika_pack::error_codes();
        let numeric: Vec<String> = nika_error::codes::ALL
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(
            canon.len() >= 30 && numeric.len() >= 30,
            "a registry read broke ({} canon rows · {} numeric codes)",
            canon.len(),
            numeric.len()
        );
        let wires = canon
            .iter()
            .map(|row| row.code)
            .chain(numeric.iter().map(String::as_str));
        let mut asking = Vec::new();
        for wire in wires {
            for door in [Door::Cli, Door::Oracle] {
                let (out, asked) = run_counting(wire, door, Some(SEAT));
                assert_eq!(out.code, exit::OK, "{wire}:\n{}", out.text);
                if asked == 0 {
                    assert!(
                        !out.text.contains(SEAT),
                        "{wire} shows a line it never asked for:\n{}",
                        out.text
                    );
                } else {
                    asking.push((wire, door, asked));
                }
            }
        }
        assert_eq!(
            asking,
            [
                ("NIKA-INFER-001", Door::Cli, 1),
                ("NIKA-INFER-001", Door::Oracle, 1),
                ("NIKA-1800", Door::Cli, 1),
                ("NIKA-1800", Door::Oracle, 1),
            ]
        );
    }

    /// The rungs outside the two registries (a hint kind · a resident
    /// code · a retired code · a namespace · the pin family · the 404)
    /// and the bare forms of an ordinary code never ask either.
    #[test]
    fn the_other_rungs_never_ask_for_the_seat_line() {
        for wire in [
            "jq-as-map",
            "native-first/006",
            "snapshot_tampered",
            "NIKA-DAG-003",
            "NIKA-PROVIDER-001",
            "NIKA-BUILTIN-WRITE-001",
            "NIKA-MCP-006",
            "NIKA-ZZZ-999",
            "DAG-002",
            "440",
        ] {
            let (out, asked) = run_counting(wire, Door::Cli, Some("SEAT-LINE"));
            assert_eq!(asked, 0, "{wire} asked for the seat line:\n{}", out.text);
            assert!(!out.text.contains("SEAT-LINE"), "{wire}:\n{}", out.text);
            assert_eq!(out.text, run(wire).text, "{wire} reads the same either way");
        }
    }

    /// The two pages that print the seat line keep their bytes: the page
    /// a machine with no seat reads, then the line on its own indented
    /// row. The bare forms ride the same gate, through both doors.
    #[test]
    fn the_seat_line_pages_keep_their_bytes() {
        const SEAT: &str = "or use a seat present on this machine: `--access codex` (its login is \
                            judged at run)";
        for wire in ["NIKA-INFER-001", "INFER-001", "NIKA-1800", "1800"] {
            for door in [Door::Cli, Door::Oracle] {
                let (bare, asked_bare) = run_counting(wire, door, None);
                let (seated, asked_seated) = run_counting(wire, door, Some(SEAT));
                assert_eq!((asked_bare, asked_seated), (1, 1), "{wire}");
                assert_eq!(bare.code, exit::OK, "{wire}:\n{}", bare.text);
                assert_eq!(seated.code, exit::OK, "{wire}:\n{}", seated.text);
                assert!(!bare.text.contains("--access codex"), "{}", bare.text);
                assert_eq!(
                    seated.text,
                    format!("{}\n  {SEAT}\n", bare.text),
                    "{wire}: the seat line is the last row, nothing else moves"
                );
            }
        }
    }

    #[test]
    fn explain_shares_value_routing_and_exec_shape_teaching() {
        let values = run("NIKA-VALUES-001");
        let shared = nika_schema::error::DeadForm::Vars.field_teaching();
        assert!(values.text.contains(&shared), "{}", values.text);
        let parse = run("NIKA-PARSE-019");
        for lesson in ["command: [", "shell:", "nika check --fix"] {
            assert!(parse.text.contains(lesson), "{}", parse.text);
        }
        let oracle = super::run_for(
            "NIKA-VALUES-001",
            Theme::new(false, false, false),
            Door::Oracle,
        );
        assert!(oracle.text.contains("`nika_check` with `fix: true`"));
        assert!(!oracle.text.contains("`nika check --fix`"));
    }

    /// R4 — the credential refusal teaches the seat escape hatch: the
    /// static fix names `--access claude-code` (the census-derived tail
    /// is gated in the host's `with_seat_tail`, pinned separately).
    #[test]
    fn var_005_explain_uses_the_parser_owned_callable_lesson() {
        let out = run("NIKA-VAR-005");
        assert_eq!(out.code, exit::OK);
        let lesson =
            nika_error::codes::spec_contract_help("NIKA-VAR-005").expect("expression lesson");
        assert!(out.text.contains(lesson), "{}", out.text);
        for callable in [
            "size(x)",
            "has(x)",
            "x.size()",
            "x.contains(s)",
            "x.startsWith(s)",
            "x.endsWith(s)",
        ] {
            assert!(out.text.contains(callable), "{}", out.text);
        }
    }

    #[test]
    fn infer_001_teaches_the_seat_escape() {
        let out = run("NIKA-INFER-001");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(
            out.text.contains("--access claude-code"),
            "the seat door is taught:\n{}",
            out.text
        );
        assert!(
            out.text.contains("custody is the process env"),
            "custody is named:\n{}",
            out.text
        );
    }

    /// Mutation pins for the tail gate: only the auth-class codes earn
    /// the tail, and only when the census found a seat (inverting the
    /// match or dropping the `None` arm must redden this).
    #[test]
    fn the_seat_tail_is_gated_on_code_class_and_census_truth() {
        let text = "body".to_owned();
        let with = crate::probe::with_seat_tail(
            "NIKA-INFER-001",
            Some(
                "or use a seat present on this machine: `--access claude-code` (its login is judged at run)",
            ),
            text.clone(),
        );
        assert!(with.contains("--access claude-code"), "{with}");
        assert!(with.starts_with("body"), "{with}");
        // NIKA-1800 rides the same gate.
        assert!(
            crate::probe::with_seat_tail("NIKA-1800", Some("tail"), text.clone()).contains("tail"),
            "the admission refusal earns the tail too"
        );
        // Another class never does, even with a seat present.
        assert_eq!(
            crate::probe::with_seat_tail("NIKA-DAG-002", Some("tail"), text.clone()),
            "body"
        );
        // No seat, no tail — the teaching never promises a phantom path.
        assert_eq!(
            crate::probe::with_seat_tail("NIKA-INFER-001", None, text.clone()),
            "body"
        );
    }

    #[test]
    fn numeric_registry_codes_answer_exit_zero() {
        let out = run("NIKA-440");
        assert_eq!(out.code, exit::OK);
        assert!(out.text.contains("NIKA-440"));
    }

    /// #1396 — `explain NIKA-EXEC-001` promised what check's own EXEC
    /// row disowns. The closer now draws the literal/run line and says
    /// an exit status is never check's.
    #[test]
    fn the_exec_closer_hands_the_exit_status_to_the_run() {
        let exec = run("NIKA-EXEC-001");
        assert_eq!(exec.code, exit::OK);
        assert!(
            !exec.text.contains("catches this before a run ever starts"),
            "no blanket promise over a run-judged class:\n{}",
            exec.text
        );
        assert!(
            exec.text.contains("exit status") && exec.text.contains("RUN's verdict"),
            "the honest split is taught:\n{}",
            exec.text
        );
    }

    /// NIK-402 · a code check cannot raise never closes on « `nika check` catches this »:
    /// INFER-002 is taught as a run verdict with its repair and its retry, the run-only and
    /// partly-checked classes say what check does catch, and an unraised row says so. Every
    /// other canon row keeps the strong closer.
    #[test]
    fn a_code_check_cannot_raise_is_taught_as_the_runs() {
        let schema = run("NIKA-INFER-002");
        assert_eq!(schema.code, exit::OK);
        assert!(
            schema.text.contains(
                "— a RUN verdict: `nika check` reads the `schema:` itself, never an answer"
            ) && schema
                .text
                .contains("`on_codes: [NIKA-INFER-002]` in a `retry:` block")
                && schema
                    .text
                    .contains("(`partial_output`) and every violation"),
            "{}",
            schema.text
        );
        let run_only = [
            "NIKA-INFER-005",
            "NIKA-EXEC-002",
            "NIKA-AGENT-001",
            "NIKA-AGENT-002",
            "NIKA-AGENT-005",
            "NIKA-AGENT-006",
            "NIKA-AGENT-007",
            "NIKA-SEC-002",
            "NIKA-SEC-003",
            "NIKA-SEC-010",
            "NIKA-SEC-011",
            "NIKA-TIMEOUT-001",
            "NIKA-CANCEL-001",
            "NIKA-MCP-001",
            "NIKA-MCP-002",
            "NIKA-DECIDE-001",
            "NIKA-DECIDE-002",
            "NIKA-TYPE-101",
            "NIKA-VAR-002",
            "NIKA-VAR-004",
            "NIKA-VAR-009",
        ];
        let unraised = [
            "NIKA-PARSE-004",
            "NIKA-TYPE-005",
            "NIKA-VAR-007",
            "NIKA-PORT-001",
            "NIKA-PORT-002",
            "NIKA-ASSERT-001",
            "NIKA-LOCK-001",
        ];
        for (codes, closer) in [
            (
                &run_only[..],
                "a RUN verdict: this engine raises it while the workflow runs, and a clean \
                 `nika check` does not rule it out.",
            ),
            (
                &unraised[..],
                "this engine raises it neither at check nor at run yet: the spec reserves or \
                 defines the row ahead of the engine.",
            ),
        ] {
            for code in codes {
                let out = run(code);
                assert_eq!(out.code, exit::OK, "{code} is a canon row");
                assert!(
                    out.text.ends_with(&format!("{code} — {closer}\n")),
                    "{code}:\n{}",
                    out.text
                );
            }
        }
        for (code, partial) in [
            (
                "NIKA-INFER-004",
                "a blank answer under any other cap is the RUN's verdict.",
            ),
            (
                "NIKA-VAR-006",
                "every other type error at evaluation is the RUN's verdict.",
            ),
        ] {
            let out = run(code);
            assert!(
                out.text.ends_with(&format!("{partial}\n"))
                    && !out.text.contains("catches this before a run ever starts"),
                "{code}:\n{}",
                out.text
            );
        }
        // Every row these lists name is still in the canon: a renamed code fails here.
        let rows = nika_pack::error_codes();
        for code in run_only.iter().chain(&unraised).chain(&["NIKA-INFER-002"]) {
            assert!(
                rows.iter().any(|row| row.code == *code),
                "{code} left the canon"
            );
        }
    }

    /// V7-2 (wave-3 · 4 personas · Priya BLOCKER): the closing claim is
    /// TRUE per code class. SEC-004's closer stops promising what check
    /// cannot judge (a computed path is the run's) — Marta read the old
    /// line under a green-check-red-run pair and « stopped running check
    /// at all ». A statically-caught class keeps the strong closer.
    #[test]
    fn the_closer_never_promises_more_than_the_judge_checked() {
        let sec = run("NIKA-SEC-004");
        assert_eq!(sec.code, exit::OK);
        assert!(
            !sec.text.contains("catches this before a run ever starts"),
            "the over-claim is gone from the run-judged class:\n{}",
            sec.text
        );
        assert!(
            sec.text.contains("judged at RUN") && sec.text.contains("not its promise"),
            "the honest split is taught:\n{}",
            sec.text
        );
        let dag = run("NIKA-DAG-002");
        assert_eq!(dag.code, exit::OK);
        assert!(
            dag.text.contains("catches this before a run ever starts"),
            "a statically-caught class keeps its strong closer:\n{}",
            dag.text
        );
        // Wave-4 founder-fr: the wire class must not promise the dial
        // check never made — the closer names the RUN's verdict and the
        // door that DOES dial ahead.
        let infer = run("NIKA-INFER-001");
        assert_eq!(infer.code, exit::OK);
        assert!(
            !infer.text.contains("catches this before a run ever starts"),
            "the over-claim is gone from the wire class:\n{}",
            infer.text
        );
        assert!(
            infer.text.contains("doctor --ping") && infer.text.contains("RUN's verdict"),
            "the honest wire split is taught:\n{}",
            infer.text
        );
        // #605: the exec floor IS statically judged now (the same
        // predicate both sides) — but only for a LITERAL argv, so the
        // closer claims exactly that and hands the templated half to RUN.
        let floor = run("NIKA-SEC-001");
        assert_eq!(floor.code, exit::OK);
        assert!(
            !floor.text.contains("catches this before a run ever starts"),
            "no blanket promise while a templated argv is run-judged:\n{}",
            floor.text
        );
        assert!(
            floor.text.contains("literal argv") && floor.text.contains("judged at RUN"),
            "the literal/templated split is taught:\n{}",
            floor.text
        );
        assert!(
            floor.text.contains("permits.fs.read")
                && floor.text.contains("permits.fs.write")
                && floor.text.contains("OS access")
                && floor.text.contains("unknown")
                && !floor.text.contains("add that path"),
            "the confined-file closer checks authority, it does not prescribe a grant:\n{}",
            floor.text
        );
    }

    /// #1592 · a seatbelt refusal reads « seatbelt refused the confined
    /// process (status 1) » at run, and the page it points at named no OS
    /// sandbox. It names the two backends this binary selects and the doctor
    /// row that reports them. Review round 1: the engine classifies a denied
    /// exit by its stderr text alone (`stderr_signals_confinement_denial`),
    /// so a file the OS refuses inside the grant reads the same — the page
    /// says « classified as », never « is », and sends the reader to the
    /// file's own permissions too.
    #[test]
    fn the_sec_001_page_names_the_os_sandbox() {
        let floor = run("NIKA-SEC-001");
        assert_eq!(floor.code, exit::OK);
        for fact in [
            "Seatbelt (`/usr/bin/sandbox-exec`) on macOS",
            "bubblewrap (`/usr/bin/bwrap`) on Linux",
            "`nika doctor` names",
            "is classified as the sandbox's refusal",
            "a program's own permission error",
            "reads the same",
            "the file's own permissions included",
        ] {
            assert!(
                floor.text.contains(fact),
                "`{fact}` missing:\n{}",
                floor.text
            );
        }
        assert!(
            !floor.text.contains("not the program's own failure"),
            "a stderr match is a classification, never proof:\n{}",
            floor.text
        );
    }

    /// #1582 · a `nika:fetch` 3xx onto another host died SEC-004 (or SEC-005
    /// for an undeclared loopback name) at run, and neither page named a
    /// redirect. Both now say the hop is held to the same boundary, and the
    /// SEC-005 closer stops promising a check-time catch of a run-time hop.
    #[test]
    fn a_redirect_hop_is_held_to_the_same_boundary_on_both_pages() {
        let boundary = run("NIKA-SEC-004");
        assert!(
            boundary
                .text
                .contains("A redirect is held to the same boundary")
                && boundary.text.contains("reached by a redirect from")
                && boundary.text.contains("redirect hop is judged at RUN"),
            "{}",
            boundary.text
        );
        let floor = run("NIKA-SEC-005");
        assert_eq!(floor.code, exit::OK);
        assert!(
            floor.text.contains("A redirect is held to the same floor")
                && floor
                    .text
                    .contains("a hop to a loopback name the file did not declare")
                && floor
                    .text
                    .contains("a redirect hop and a host that resolves"),
            "{}",
            floor.text
        );
        assert!(
            !floor.text.contains("catches this before a run ever starts"),
            "a run-time hop is never a check-time promise:\n{}",
            floor.text
        );
    }

    /// The SEC-009 page described the trifecta but printed no fix, and the
    /// finding's own fix names only the gate. Both doors now teach dropping
    /// a leg before the gate, and say what the gate is consent to.
    #[test]
    fn the_trifecta_page_teaches_the_leg_before_the_gate() {
        let cli = run("NIKA-SEC-009");
        assert_eq!(cli.code, exit::OK);
        let leg = cli
            .text
            .find("fix: first drop a leg the workflow does not need");
        let gate = cli
            .text
            .find("add a blocking `nika:prompt` (no `default:`, granted in `permits.tools`)");
        assert!(
            matches!((leg, gate), (Some(leg), Some(gate)) if leg < gate),
            "the leg comes first, then the gate:\n{}",
            cli.text
        );
        for fact in [
            "private read · `permits.fs.read`",
            "untrusted ingress · `nika:fetch` or an `mcp:` server in `permits.tools`, or `exec`",
            "external egress · `permits.net.http`, an escaping `fs.write` glob or `exec`",
            "an exec grant arms two legs",
            "safe by construction",
            "every path to the egress task crosses, data edges included",
            "Placed after the reads and fetches, its question can show the content",
            "upstream of every entry task the finding names",
            "consents to the run, not to the payload",
            "into each effect that runs after it, a fetch included",
            "gate that effect with `when:`",
            "NIKA-SEC-014 refuses an effect it can prove runs on a « no » to a confirm gate",
            "Only a `mode: confirm` gate counts by its position alone",
            "A `mode: choice` or `mode: input` gate answers with text, not a yes",
            "it counts only before an effect whose own `when:` tests that answer",
            "or on the output of a `nika:jq` stage whose program reads it",
            "an effect that only waits on it runs whatever the answer",
            "An input gate also counts before a `nika:` tool call that sends only the typed answer",
            "A gate of any mode, a confirm included, with an `on_error:` or a `for_each:` counts before no effect",
        ] {
            assert!(cli.text.contains(fact), "`{fact}` missing:\n{}", cli.text);
        }
        // The page names only the tools the check counts as ingress.
        assert!(!cli.text.contains("browsing"), "{}", cli.text);
        let oracle = run_for(
            "NIKA-SEC-009",
            Theme::new(false, false, false),
            Door::Oracle,
        );
        assert_eq!(oracle.code, exit::OK);
        assert!(
            oracle.text.contains("fix: first drop a leg"),
            "the MCP door teaches the same fix:\n{}",
            oracle.text
        );
    }

    #[test]
    fn spec_conformance_codes_answer_from_the_canon() {
        // ONE voice: every code `nika check` emits is explainable.
        let out = run("NIKA-VAR-021");
        assert_eq!(
            out.code,
            exit::OK,
            "spec codes must teach, not 404:\n{}",
            out.text
        );
        assert!(out.text.contains("NIKA-VAR-021"));
        assert!(out.text.contains("validation_error"));
        assert!(
            out.text
                .contains("https://nika.sh/language/errors/NIKA-VAR-021"),
            "the footer links the code's own page:\n{}",
            out.text
        );
        assert!(
            out.text.contains("hoist the reference into `with:`"),
            "the fix-form states the concrete YAML edit (#145 P1):\n{}",
            out.text
        );
    }

    #[test]
    fn retired_codes_teach_the_retirement_not_a_404() {
        // NIKA-DAG-003 died in W2 « the flow » (the binding IS the edge);
        // NIKA-PARSE-016 folded into NIKA-VAR-005. Old traces and docs
        // still name them — the answer is the retirement, never a guess.
        let out = run("NIKA-DAG-003");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(out.text.contains("retired"), "{}", out.text);
        assert!(
            out.text.contains("NIKA-VAR-021"),
            "points at the successor class:\n{}",
            out.text
        );
        assert!(
            out.text.contains("the `with:` binding IS the edge"),
            "teaches the W2 law that made it inexpressible:\n{}",
            out.text
        );
        let folded = run("NIKA-PARSE-016");
        assert_eq!(folded.code, exit::OK, "{}", folded.text);
        assert!(folded.text.contains("retired"), "{}", folded.text);
        assert!(folded.text.contains("NIKA-VAR-005"), "{}", folded.text);
        // The test's own name, finally enforced: the docs door taught is
        // the SUCCESSOR's page — a retired code's per-code URL is a 404
        // (measured 2026-08-01 · the site projects the CURRENT canon
        // table, and retirement is exactly the row it no longer has).
        assert!(
            out.text.contains("errors/NIKA-VAR-021"),
            "the taught URL is the live successor page:\n{}",
            out.text
        );
        assert!(
            !out.text.contains("errors/NIKA-DAG-003"),
            "never the retired 404:\n{}",
            out.text
        );
    }

    #[test]
    fn parse_002_teaches_the_nine_key_minimum() {
        // Leftover fourteen-key teaching (`nika: v1` + `workflow:` + a
        // `tasks:` list) would send the author back to a dialect this
        // binary refuses. The minimum is two things: identity on `nika:`
        // and a non-empty `tasks:` map — never a third required key.
        let out = run("NIKA-PARSE-002");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(
            out.text.contains("`nika: <kebab-id>`"),
            "identity lives on nika:\n{}",
            out.text
        );
        assert!(
            out.text.contains("`tasks:` map"),
            "tasks is a map, not a list:\n{}",
            out.text
        );
        assert!(
            out.text.contains("`#` comment"),
            "description is a comment, not a key:\n{}",
            out.text
        );
        assert!(
            !out.text.contains("nika: v1"),
            "the version slot is gone:\n{}",
            out.text
        );
        assert!(
            !out.text.contains("workflow: <name>") && !out.text.contains("three lines"),
            "do not revive the fourteen-key trio:\n{}",
            out.text
        );
    }

    #[test]
    fn parse_005_teaches_the_closed_envelope_not_description() {
        // `description:` died with the envelope nuke — unknown fields are
        // not parked there; custom prose is a `#` comment above `nika:`.
        let out = run("NIKA-PARSE-005");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(
            out.text.contains("closed envelope"),
            "names the closed set:\n{}",
            out.text
        );
        assert!(
            out.text.contains("`#` comment") && out.text.contains("`nika:`"),
            "custom prose is a comment above nika:\n{}",
            out.text
        );
        assert!(
            !out.text.contains("description:"),
            "description: is dead:\n{}",
            out.text
        );
    }

    #[test]
    fn parse_005_teaches_the_column_not_the_schema() {
        // #1622: a first-minute author sent to `nika spec --schema` reads a
        // JSON Schema to learn what the finding already listed. The fix names
        // the listed column and maps the dialect keys to their verbs.
        let out = run("NIKA-PARSE-005");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(
            out.text.contains("infer:") && out.text.contains("nika:fetch"),
            "names the verbs that own the dialect keys:\n{}",
            out.text
        );
        assert!(
            !out.text.contains("nika spec --schema"),
            "the schema dump is not the fix:\n{}",
            out.text
        );
    }

    #[test]
    fn parse_024_names_both_w2_doors_and_the_codemod() {
        // The dead-form code is the highest-traffic migration surface:
        // its fix-form must name the two doors (`with:` data · `after:`
        // control) and the machine path (`nika check --fix`).
        let out = run("NIKA-PARSE-024");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(out.text.contains("depends_on"), "{}", out.text);
        assert!(out.text.contains("`with:`"), "{}", out.text);
        assert!(out.text.contains("after:"), "{}", out.text);
        assert!(out.text.contains("nika check --fix"), "{}", out.text);
    }

    #[test]
    fn var001_teaches_the_var_flag() {
        // F4: `nika explain NIKA-VAR-001` must say HOW to supply a var
        // from the CLI, not just that the reference is unresolved.
        let out = run("NIKA-VAR-001");
        assert_eq!(out.code, exit::OK);
        assert!(out.text.contains("--var"), "names the flag:\n{}", out.text);
        assert!(
            out.text.contains("default:"),
            "names the workflow-side fix too:\n{}",
            out.text
        );
    }

    /// #1599 · a bare `${{ fetch.output }}` is the other common VAR-001:
    /// the page teaches the task-output form through `with:`, and never
    /// the `config:` root the nine-key envelope refuses (PARSE-005).
    #[test]
    fn var001_teaches_the_task_output_form_and_no_dead_config_root() {
        let out = run("NIKA-VAR-001");
        assert_eq!(out.code, exit::OK);
        assert!(
            out.text.contains("`tasks.<id>.output`")
                && out
                    .text
                    .contains("with: { page: \"${{ tasks.fetch.output }}\" }")
                && out.text.contains("`${{ with.page }}`"),
            "the task-output form, bound in with:, is taught:\n{}",
            out.text
        );
        assert!(
            !out.text.contains("config:"),
            "a dead envelope key is never taught as a root:\n{}",
            out.text
        );
    }

    #[test]
    fn bare_prefixed_form_normalizes() {
        let out = run("DAG-005");
        assert_eq!(out.code, exit::OK);
        assert!(out.text.contains("NIKA-DAG-005"));
    }

    #[test]
    fn every_canon_table_row_explains() {
        // DERIVED coverage (never a hand-enumerated list): every code
        // the typed registry carries must answer exit 0. The parse
        // contract itself (anchoring · escape-free rows · count vs the
        // canon's own field) is pinned in nika-pack's seam tests.
        let rows = nika_pack::error_codes();
        assert!(
            rows.len() >= 30,
            "registry parse broke ({} rows)",
            rows.len()
        );
        for row in rows {
            let out = run(row.code);
            assert_eq!(
                out.code,
                exit::OK,
                "{} must explain:\n{}",
                row.code,
                out.text
            );
        }
    }

    /// NIK-254 · a run refused with `NIKA-MCP-006` prints `fix: nika explain NIKA-MCP-006`:
    /// the pin family the engine mints is answered with its remedy, never the 404, by both
    /// doors.
    #[test]
    fn the_mcp_pin_family_teaches_its_remedy() {
        for (wire, code, remedy) in [
            ("NIKA-MCP-003", "NIKA-MCP-003", "nika mcp approve"),
            ("NIKA-MCP-004", "NIKA-MCP-004", "nika mcp approve"),
            ("NIKA-MCP-005", "NIKA-MCP-005", "sandbox"),
            ("NIKA-MCP-006", "NIKA-MCP-006", "nika mcp approve"),
            ("MCP-006", "NIKA-MCP-006", "nika mcp approve"),
        ] {
            for door in [Door::Cli, Door::Oracle] {
                let out = super::run_for(wire, Theme::new(false, false, false), door);
                assert_eq!(out.code, exit::OK, "{wire}:\n{}", out.text);
                assert!(out.text.starts_with(code), "{wire}:\n{}", out.text);
                assert!(out.text.contains(remedy), "{wire}:\n{}", out.text);
                assert!(!out.text.contains("unknown code"), "{wire}:\n{}", out.text);
            }
        }
        // The tripwire: once the canon carries these rows, its rung answers them and
        // `mcp_pin_help` is deleted.
        let rows = nika_pack::error_codes();
        for code in [
            "NIKA-MCP-003",
            "NIKA-MCP-004",
            "NIKA-MCP-005",
            "NIKA-MCP-006",
        ] {
            assert!(
                rows.iter().all(|row| row.code != code),
                "{code} is a canon row now: delete `mcp_pin_help`"
            );
        }
    }

    /// #1608 · the checklist names the causes the refusal itself names on the pointed line:
    /// a surplus `: `, an unquoted expression inside a flow mapping, an unclosed flow.
    #[test]
    fn parse_001_names_the_surplus_colon_and_the_flow_expression() {
        let out = run("NIKA-PARSE-001");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        for needle in [
            "a `: ` inside a plain value",
            "quote the whole value",
            "inside a flow mapping",
            "quote the expression (`{ prompt: \"${{ inputs.t }}\" }`)",
            "the file never closes",
            "a key missing its `:`",
            "`# yaml-language-server:` modeline",
        ] {
            assert!(out.text.contains(needle), "{needle}:\n{}", out.text);
        }
    }

    #[test]
    fn unknown_codes_stay_a_finding() {
        let out = run("NIKA-ZZZ-999");
        assert_eq!(out.code, exit::FILE);
        assert!(out.text.contains("unknown code"));
        assert!(
            out.text.contains("[brackets]"),
            "the 404 names the other occupant of the slot:\n{}",
            out.text
        );
    }

    /// V9 wave 3 · row 11 (p01): `nika explain` refused a mistyped code
    /// with a range list and no way to search — « if a sceptic mistyped a
    /// code, they'd be stuck ». A near-miss now names its neighbour, and a
    /// code that is near NOTHING still says nothing (a wrong suggestion is
    /// worse than none · the shared `did_you_mean` threshold decides).
    #[test]
    fn a_mistyped_code_is_answered_with_its_nearest_neighbour() {
        let real = nika_pack::error_codes();
        let known = real.first().expect("the canon ships error codes").code;
        // One deletion from a real code — the dominant typo class.
        let typo = &known[..known.len() - 1];
        let out = run(typo);
        assert_eq!(out.code, exit::FILE, "{}", out.text);
        assert!(
            real.iter()
                .any(|r| out.text.contains(&format!("did you mean `{}`?", r.code))),
            "the neighbour of `{typo}` is named, and it is a REAL canon code:\n{}",
            out.text
        );
        // Near nothing → the 404 alone, never an invented neighbour.
        let far = run("NIKA-ZZZ-999");
        assert!(
            !far.text.contains("did you mean"),
            "a far code earns silence:\n{}",
            far.text
        );
    }

    #[test]
    fn a_printed_hint_kind_teaches_instead_of_404() {
        // #1038 · check prints `[jq-as-map]` in the code slot; explain
        // used to wrap it as `NIKA-jq-as-map` and refuse.
        let out = run("jq-as-map");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(out.text.starts_with("jq-as-map · hint"), "{}", out.text);
        assert!(out.text.contains("($name | map"), "{}", out.text);
        assert!(out.text.contains("paid_ready"), "{}", out.text);
    }

    #[test]
    fn a_numbered_native_first_rule_teaches() {
        let out = run("native-first/006");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(
            out.text.starts_with("native-first/006 · hint"),
            "{}",
            out.text
        );
        assert!(out.text.contains("nika:wait"), "{}", out.text);
    }

    #[test]
    fn provider_namespace_codes_teach_not_404() {
        // A per-provider code (NIKA-PROVIDER-NNN · valid in on_codes:) must
        // EXPLAIN its namespace — symmetric with per-builtin — not flat-404.
        let out = run("NIKA-PROVIDER-001");
        assert_eq!(
            out.code,
            exit::OK,
            "provider namespace must teach:\n{}",
            out.text
        );
        assert!(out.text.contains("provider"));
        assert!(out.text.contains("on_codes"));
        // a non-conforming shape (not 3 digits) stays unknown
        let bad = run("NIKA-PROVIDER-1");
        assert_eq!(bad.code, exit::FILE, "{}", bad.text);
    }

    #[test]
    fn builtin_namespace_codes_teach_not_404() {
        // NEW-3a: the per-builtin runtime code the nika:write null-guard
        // emits must EXPLAIN (builtin name + on_codes usability), not the
        // flat "unknown code" — ONE voice for every emitted code.
        let out = run("NIKA-BUILTIN-WRITE-001");
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(out.text.contains("nika:write"), "{}", out.text);
        assert!(out.text.contains("on_codes"), "{}", out.text);
        // underscore-named builtins normalize too (json_merge_patch).
        assert!(
            run("NIKA-BUILTIN-JSON_MERGE_PATCH-001")
                .text
                .contains("nika:json_merge_patch")
        );
        // a malformed builtin code (empty name) stays a finding.
        assert_eq!(run("NIKA-BUILTIN--001").code, exit::FILE);
    }

    #[test]
    fn commas_inside_the_failure_text_render_intact() {
        // The parse detail lives in nika-pack now; this pins the
        // CONSUMER-visible property — VAR-002's comma-bearing failure
        // text arrives whole through the typed seam.
        let out = run("NIKA-VAR-002");
        assert_eq!(out.code, exit::OK);
        assert!(out.text.contains("zero or multiple values"), "{}", out.text);
    }

    /// On the linked register the doc-site reference rides the OSC-8
    /// wrapper (scheme in the URL · prose text unchanged); the sober
    /// register — every pipe — keeps its exact bytes, zero escapes.
    #[test]
    fn doc_site_reference_links_on_the_linked_register() {
        let mut linked = Theme::new(false, false, false);
        linked.links = true;
        for wire in [
            "NIKA-BUILTIN-WRITE-001",
            "NIKA-PROVIDER-001",
            "NIKA-ZZZ-999",
        ] {
            let out = super::run(wire, linked);
            assert!(
                out.text.contains(
                    "\x1b]8;;https://docs.nika.sh/errors\x1b\\docs.nika.sh/errors\x1b]8;;\x1b\\"
                ),
                "{wire} links the doc site: {:?}",
                out.text
            );
            let sober = run(wire);
            assert!(
                !sober.text.contains('\x1b'),
                "{wire} sober register is escape-free"
            );
        }
    }
}
