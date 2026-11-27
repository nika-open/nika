// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What Nika can hold a harness seat's `agent:` session to (NIK-266) — the ONE table the
//! client (which selects and reads back the mode) and `nika check` (which tells the author
//! what the boundary covers) both read, so the two can never drift.
//!
//! A harness runs its OWN tools. Nika's authority over it is two levers: the session mode it
//! selects before the prompt, and its answer to every permission the harness asks. A mode is
//! a row here only when it was MEASURED on a named adapter release (its source and its
//! `session/new` answer) to send its approvals to the client; a mode that approves on its own
//! (an auto-reviewer, accept-edits, full access, bypass, yolo) is never a row. What the
//! measured mode still leaves to the harness itself, without asking, is stated in the row —
//! Nika cannot see or judge it.
//!
//! A row is found by the adapter's reported name, and holds the releases it was measured on
//! ([`HarnessBoundary::releases`]). A session whose adapter reports another release keeps the
//! row's rule (the mode is selected and read back, a command ask on Codex still leaves its
//! sandbox), and every text that states what the harness does without asking says that it
//! was measured on another release ([`HarnessBoundary::for_release`]).

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;

use super::release::{AdapterRelease, shown};

/// One measured confinement of a harness adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct HarnessBoundary {
    /// The seat token this adapter serves ([`super::HarnessRuntime::id`]).
    pub seat: &'static str,
    /// The `agentInfo.name` values the measurement holds for — the adapter's own report on
    /// `initialize`, never the command that spawned it.
    pub agents: &'static [&'static str],
    /// The adapter release the measurement was made on, as a person reads it.
    pub measured_on: &'static str,
    /// The adapter releases the measurement was made on, compared with the release the
    /// adapter reports on `initialize` (`agentInfo.version`).
    pub releases: &'static [AdapterRelease],
    /// The mode selected when the workflow grants no `fs.write`.
    pub read_only_mode: &'static str,
    /// The mode selected when the workflow grants `fs.write`.
    pub workspace_write_mode: &'static str,
    /// What the harness reads without asking, under the selected mode.
    pub reads_unasked: &'static str,
    /// Where it writes without asking, under the selected mode (`None`: every write asks).
    pub writes_unasked: Option<&'static str>,
    /// What it runs without asking, under the selected mode (`None`: every command asks).
    pub runs_unasked: Option<&'static str>,
    /// What it fetches without asking, under the selected mode (`None`: every fetch asks).
    pub fetches_unasked: Option<&'static str>,
    /// What it loads or applies on its own at every session, and what the client keeps it
    /// from loading (`None`: nothing measured).
    pub loads_unasked: Option<&'static str>,
    /// Whether the selected mode runs ordinary commands in the harness's own sandbox without
    /// asking, so a command it asks about asks to LEAVE that sandbox (NIK-274): the client
    /// sends such an ask as an escalation, which no grant answers, `exec: true` included.
    pub asked_command_leaves_sandbox: bool,
    /// Whether the client recognises this adapter's image tool, so an image the harness
    /// generates reaches the task with its bytes through `images:` (spec 02 §Harness images).
    /// Only a recognised tool's bytes are received; on another adapter `images: { save: }`
    /// can only end in `NIKA-AGENT-007`.
    pub delivers_images: bool,
    /// Whether the adapter selects a session's model only through its model configuration
    /// option. A model a task names that the option does not offer then refuses before the
    /// prompt: the client never falls back to the legacy `session/set_model` (each row says
    /// why), on any release of the adapter.
    pub models_through_option: bool,
}

/// The release an adapter reported, when it reported one: an empty or blank text is none.
fn reported_release(reported: Option<&str>) -> Option<&str> {
    reported.filter(|release| !release.trim().is_empty())
}

/// A measured row as it holds for one session: the row found by the name the adapter
/// reported on `initialize`, and whether it was measured on the release it reported
/// ([`HarnessBoundary::for_release`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SessionBoundary {
    /// The row was measured on the reported release: its text stands as written.
    Measured(HarnessBoundary),
    /// The row was not measured on the reported release (another release, one the client
    /// cannot read, or none reported). Its rule holds unchanged, and its text is published
    /// with [`HarnessBoundary::carried_note`].
    Carried {
        /// The row of the adapter's name.
        row: HarnessBoundary,
        /// The release the adapter reported (`None`: it reported none, or an empty or blank
        /// text), the adapter's own text, unfiltered: print it through
        /// [`HarnessBoundary::carried_note`] or [`SessionBoundary::note`], which repeat it as
        /// bounded printable ASCII.
        reported: Option<String>,
    },
}

impl SessionBoundary {
    /// The row, measured or carried: its modes and its stricter readings hold either way.
    #[must_use]
    pub const fn row(&self) -> HarnessBoundary {
        match self {
            Self::Measured(row) | Self::Carried { row, .. } => *row,
        }
    }

    /// Whether the row was measured on the release the session's adapter reported.
    #[must_use]
    pub const fn is_measured(&self) -> bool {
        matches!(self, Self::Measured(_))
    }

    /// The sentence a carried session adds to what it states ([`HarnessBoundary::carried_note`]):
    /// the release measured and the one the adapter reports. `None` on a measured session.
    #[must_use]
    pub fn note(&self) -> Option<String> {
        match self {
            Self::Measured(_) => None,
            Self::Carried { row, reported } => Some(row.carried_note(reported.as_deref())),
        }
    }

    /// What the harness still does without asking, as the run's witness states it: the
    /// row's text ([`HarnessBoundary::unasked`]), followed on a carried session by the
    /// sentence that names both releases ([`Self::note`]).
    #[must_use]
    pub fn unasked(&self, writes_granted: bool) -> String {
        let text = self.row().unasked(writes_granted);
        match self.note() {
            Some(note) => format!("{text} {note}"),
            None => text,
        }
    }
}

impl HarnessBoundary {
    /// codex-acp 1.13.1 (it bundles codex 0.156.1). Its three modes, read from its source and
    /// its `session/new` answer: `read-only` ("Ask for approval": approval policy `on-request`,
    /// reviewed by the user — the client — over a `workspaceWrite` sandbox: the session root,
    /// `/tmp` and `$TMPDIR` writable, network off, reads unrestricted); `agent` (the adapter's
    /// DEFAULT, "Approve for me": the same sandbox, approvals reviewed by Codex's own
    /// auto-reviewer, so most never reach the client); `agent-full-access` (no sandbox, no
    /// approval at all). No mode is a read-only sandbox, so both confinements select
    /// `read-only`, the only mode whose every approval reaches Nika. In it, ordinary commands
    /// run sandboxed without asking, so a command ask is an escalation (`require_escalated`,
    /// the command as a shell STRING, the reason in `_meta.permission.description`): approved,
    /// it runs with no sandbox at all. A network approval is a command ask titled `<protocol>
    /// network access to <host>` (request `_meta.permission.title` `Allow network access?`),
    /// with `rawInput.url` only for http and https; `rawInput.additionalPermissions` asks to
    /// widen the sandbox. Measured with real Codex on 2026-10-26: without asking it also calls
    /// its MCP tools marked `readOnlyHint` (remote ones included), runs its tools through a
    /// JavaScript code mode, opens pages with its hosted web tool, writes its own `~/.codex`
    /// (images, session logs, state), starts every configured MCP server and applies the
    /// user's global `~/.codex/AGENTS.md` and Codex plugins.
    pub const CODEX_ACP: Self = Self {
        seat: "codex",
        agents: &["@agentclientprotocol/codex-acp", "codex-acp"],
        measured_on: "codex-acp 1.13.1",
        releases: &[AdapterRelease::new(1, 13, 1)],
        read_only_mode: "read-only",
        workspace_write_mode: "read-only",
        reads_unasked: "any file",
        writes_unasked: Some("the project, /tmp, $TMPDIR and its own ~/.codex"),
        runs_unasked: Some(
            "any program in its own sandbox, its tools through a JavaScript code mode and its \
             MCP tools marked readOnlyHint (remote ones included)",
        ),
        fetches_unasked: Some("pages with its hosted web tool, whose content reaches the model"),
        loads_unasked: Some(
            "starts every configured MCP server and applies your global ~/.codex/AGENTS.md \
             and Codex plugins",
        ),
        asked_command_leaves_sandbox: true,
        delivers_images: true,
        // Its legacy `session/set_model` sets the effort together with the model (each id it
        // lists is `model[effort]`) and answers nothing the client can read back.
        models_through_option: true,
    };

    /// claude-agent-acp 0.81.1 (Claude Code 2.1.280). Its modes: `default` ("Manual": every
    /// edit and every non-read-only command asks), `acceptEdits`, `plan`, `auto` (a classifier
    /// decides, approvals rarely reach the client) and `bypassPermissions`. A session starts in
    /// the user's `permissions.defaultMode` (`auto` on a machine that set it), so `default` is
    /// selected and read back for both confinements — `plan` still lets an allowed write run.
    /// Reads in the session root, commands it classifies read-only and pre-approved
    /// documentation hosts proceed without asking; a command ask carries a shell STRING. The
    /// client opens the session with the user's, the project's and the local settings files
    /// turned off (`settingSources: []`), strict MCP naming no server, no plugin, skill or
    /// custom agent, auto memory and claude.ai connectors off: their permission rules (allow,
    /// ask and deny alike), sandbox settings, hooks, memory files and MCP servers are not
    /// loaded. Two inputs no option reaches are read regardless, a managed policy and the
    /// global `~/.claude.json`; the adapter also reads the settings files itself for the mode
    /// it starts in (replaced by `default`), the model of a task that names none, its model
    /// list and its default effort.
    pub const CLAUDE_AGENT_ACP: Self = Self {
        seat: "claude-code",
        agents: &["@agentclientprotocol/claude-agent-acp"],
        measured_on: "claude-agent-acp 0.81.1",
        releases: &[AdapterRelease::new(0, 81, 1)],
        read_only_mode: "default",
        workspace_write_mode: "default",
        reads_unasked: "files in the project",
        writes_unasked: None,
        runs_unasked: Some("shell commands Claude Code classifies as read-only"),
        fetches_unasked: Some("documentation hosts Claude Code pre-approves"),
        loads_unasked: Some(
            "applies a managed Claude Code policy when the machine has one, and runs a task \
             that names no model on the one your Claude settings name (your own permission \
             rules, allow, ask or deny, hooks, MCP servers, plugins, skills and memory files are \
             not loaded into the session)",
        ),
        asked_command_leaves_sandbox: false,
        delivers_images: false,
        // It has no `session/set_model` handler.
        models_through_option: true,
    };

    /// gemini-cli 0.62.0. Legacy modes only (`session/set_mode`, no mode config option):
    /// `default` (edits, shell commands and fetches ask), `autoEdit`, `yolo`, `plan`. Its file
    /// tools refuse every path outside the workspace; known-safe read commands inside it and
    /// Google web searches proceed without asking; an ask carries no `rawInput` (the command
    /// and the URL ride only its title).
    pub const GEMINI_CLI: Self = Self {
        seat: "gemini-cli",
        agents: &["gemini-cli"],
        measured_on: "gemini-cli 0.62.0",
        releases: &[AdapterRelease::new(0, 62, 0)],
        read_only_mode: "default",
        workspace_write_mode: "default",
        reads_unasked: "files in the project",
        writes_unasked: None,
        runs_unasked: Some("shell commands it judges safe reads in the project"),
        fetches_unasked: Some("Google web searches"),
        loads_unasked: None,
        asked_command_leaves_sandbox: false,
        delivers_images: false,
        // Its model selection was not measured: the legacy list stays a way in.
        models_through_option: false,
    };

    /// Every measured adapter, in the registry's order (gemini first · Anthropic last).
    pub const MEASURED: [Self; 3] = [Self::GEMINI_CLI, Self::CODEX_ACP, Self::CLAUDE_AGENT_ACP];

    /// The measured boundary of the adapter that reported `agent_name` on `initialize`,
    /// whatever its release: [`Self::for_release`] says whether it was measured on it.
    #[must_use]
    pub fn for_agent(agent_name: &str) -> Option<Self> {
        Self::MEASURED
            .into_iter()
            .find(|row| row.agents.contains(&agent_name))
    }

    /// The boundary of the adapter that reported `agent_name` and the release `reported` on
    /// `initialize`: measured when the row was made on that release, carried otherwise (a
    /// release the client cannot read, or none, is carried, never measured). `None`: no row
    /// for that name, so the unmeasured rule applies.
    #[must_use]
    pub fn for_release(agent_name: &str, reported: Option<&str>) -> Option<SessionBoundary> {
        let row = Self::for_agent(agent_name)?;
        let reported = reported_release(reported);
        Some(match reported {
            Some(release) if row.measures(release) => SessionBoundary::Measured(row),
            _ => SessionBoundary::Carried {
                row,
                reported: reported.map(ToOwned::to_owned),
            },
        })
    }

    /// Whether the row was measured on the release an adapter reported, compared exactly
    /// with [`Self::releases`]: no range and no prefix.
    #[must_use]
    pub fn measures(&self, reported: &str) -> bool {
        AdapterRelease::measured_among(reported, self.releases)
    }

    /// The sentence a carried session adds to every text that states what the harness does
    /// without asking: the release the row was measured on, and the one the session's
    /// adapter reported (`None`, or an empty or blank text: it reported none), repeated as
    /// printable ASCII with no backtick.
    #[must_use]
    pub fn carried_note(&self, reported: Option<&str>) -> String {
        let session = match reported_release(reported) {
            Some(release) => format!(
                "this session's adapter reports release `{}`, on which it was not measured",
                shown(release)
            ),
            None => "this session's adapter reports no release, so it was not measured on \
                     the one it runs"
                .to_owned(),
        };
        format!(
            "What the harness does without asking was measured on {}; {session}.",
            self.measured_on
        )
    }

    /// The measured boundary of the seat `id` (`codex`).
    #[must_use]
    pub fn for_seat(id: &str) -> Option<Self> {
        Self::MEASURED.into_iter().find(|row| row.seat == id)
    }

    /// The mode a confinement selects: no `fs.write` grant → [`Self::read_only_mode`].
    #[must_use]
    pub const fn mode_for_writes(&self, writes: bool) -> &'static str {
        if writes {
            self.workspace_write_mode
        } else {
            self.read_only_mode
        }
    }

    /// What the harness does without asking, in one clause: reads, writes (told against the
    /// workflow's own `fs.write`), runs, fetches, loads.
    fn unasked_clause(&self, writes_granted: bool) -> String {
        let mut clause = format!("reads {}", self.reads_unasked);
        if let Some(place) = self.writes_unasked {
            let scope = if writes_granted {
                "(not only the fs.write paths)"
            } else {
                "(the workflow grants no fs.write)"
            };
            let reach = if writes_granted { "anywhere in " } else { "" };
            clause.push_str("; writes ");
            clause.push_str(reach);
            clause.push_str(place);
            clause.push(' ');
            clause.push_str(scope);
        }
        if let Some(runs) = self.runs_unasked {
            clause.push_str("; runs ");
            clause.push_str(runs);
        }
        if let Some(fetches) = self.fetches_unasked {
            clause.push_str("; fetches ");
            clause.push_str(fetches);
        }
        if let Some(loads) = self.loads_unasked {
            clause.push_str("; ");
            clause.push_str(loads);
        }
        clause
    }

    /// What the harness still does without asking under the selected mode, as the run's
    /// witness states it (`permit_checked`, plane `agent`).
    #[must_use]
    pub fn unasked(&self, writes_granted: bool) -> String {
        format!(
            "Without asking, the harness {}: permits: does not bound that. {HARNESS_SHARED_RESIDUAL}",
            self.unasked_clause(writes_granted)
        )
    }

    /// The `nika check` advice for an `agent:` task this seat serves (one line, task-free so
    /// the renderer folds every task of one seat into one row with its site count), ending
    /// with [`HARNESS_IMAGES_ADVICE`] on an adapter that [`Self::delivers_images`].
    #[must_use]
    pub fn check_advice(&self, writes_granted: bool) -> String {
        let advice = format!(
            "an `agent:` task on {} runs in mode `{}`: Nika judges each approval it asks \
             (journaled permit_checked), but without asking it {} — permits: does not bound \
             that (`nika explain harness`)",
            self.seat,
            self.mode_for_writes(writes_granted),
            self.unasked_clause(writes_granted)
        );
        if self.delivers_images {
            format!("{advice}. {HARNESS_IMAGES_ADVICE}")
        } else {
            advice
        }
    }
}

/// How an image a seat generates reaches the workflow (spec 02 §Harness images): the bytes
/// this session sent, never a copy out of a folder the harness shares with other sessions.
/// Stated only for an adapter whose image tool the client recognises
/// ([`HarnessBoundary::delivers_images`]).
pub const HARNESS_IMAGES_ADVICE: &str = "An image it generates reaches the workflow through \
     `images: { save: <path> }`: Nika writes the bytes this session sent, never a file the \
     harness reports or a copy from its shared images folder.";

/// What holds on EVERY harness seat, measured or not (NIK-266): the residual beside each row.
pub const HARNESS_SHARED_RESIDUAL: &str = "A command asked as a shell string is admitted only \
     by `permits.exec: true`, otherwise it pauses for the operator; a command Codex asks about \
     in `read-only` asks to leave its sandbox and always pauses. An ask for network access (a \
     fetch, or a command the harness says needs the network), for a wider harness sandbox or \
     to approve one of the harness's own MCP tool calls always pauses. An action Nika allows \
     once runs under the harness's own sandbox, not Nika's exec sandbox, so permits.fs and \
     permits.net do not confine it: an allowed command opens whatever connections that sandbox \
     lets it. Settings the harness loads on its own (its permission rules, hooks and MCP \
     servers) still apply: what they allow never asks.";

/// The marker an anchored ask location starts with when the client could not resolve the
/// file the harness's path reaches (a symlink loop, an unreadable component, a component that
/// is not a directory). The bridge's judge reads it as unverifiable, never as a path a grant
/// could admit, so the ask pauses for the operator (NIK-266). A real path that happens to
/// start with it only pauses too: the collision is fail-closed.
pub const HARNESS_UNRESOLVED_PATH: &str = "<unresolved>";

/// What an unmeasured harness does under its own mode, in the engine's words: Nika selects
/// nothing and judges only the asks that reach it.
pub const UNMEASURED_HARNESS_UNASKED: &str = "No approval mode is measured for this harness: \
     Nika judges only the permissions it asks for, and whatever it reads, writes, runs or \
     fetches without asking stays outside the declared boundary.";

/// The `nika check` advice for an `agent:` task on a seat with no measured boundary.
#[must_use]
pub fn unmeasured_check_advice(seat: &str) -> String {
    format!(
        "an `agent:` task on {seat} runs in a mode Nika has not measured: Nika judges only the \
         permissions it asks for (journaled permit_checked); whatever it reads, writes, runs or \
         fetches unasked is outside permits: (`nika explain harness`)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measured_adapter_is_found_by_its_own_report_only() {
        assert_eq!(
            HarnessBoundary::for_agent("@agentclientprotocol/codex-acp"),
            Some(HarnessBoundary::CODEX_ACP)
        );
        assert_eq!(
            HarnessBoundary::for_agent("codex-acp"),
            Some(HarnessBoundary::CODEX_ACP),
            "the deprecated package answers its bare name"
        );
        assert_eq!(
            HarnessBoundary::for_agent("@agentclientprotocol/claude-agent-acp"),
            Some(HarnessBoundary::CLAUDE_AGENT_ACP)
        );
        assert_eq!(
            HarnessBoundary::for_agent("gemini-cli"),
            Some(HarnessBoundary::GEMINI_CLI)
        );
        for unmeasured in [
            "Codex",
            "Claude Agent",
            "@zed-industries/claude-agent-acp",
            "asking-harness",
        ] {
            assert_eq!(HarnessBoundary::for_agent(unmeasured), None, "{unmeasured}");
        }
        assert_eq!(
            HarnessBoundary::for_seat("codex"),
            Some(HarnessBoundary::CODEX_ACP)
        );
        assert_eq!(HarnessBoundary::for_seat("kimi-code"), None);
        for row in HarnessBoundary::MEASURED {
            assert!(
                super::super::HarnessRuntime::lookup(row.seat).is_some(),
                "{} is a shipped seat",
                row.seat
            );
        }
    }

    /// Every measured row selects the mode whose approvals reach the client, for both
    /// confinements: never an auto-reviewer, accept-edits, full access, bypass or yolo.
    #[test]
    fn every_measured_row_selects_an_asking_mode() {
        let selected: alloc::vec::Vec<(&str, &str, &str)> = HarnessBoundary::MEASURED
            .iter()
            .map(|row| (row.seat, row.read_only_mode, row.workspace_write_mode))
            .collect();
        assert_eq!(
            selected,
            [
                ("gemini-cli", "default", "default"),
                ("codex", "read-only", "read-only"),
                ("claude-code", "default", "default"),
            ]
        );
        for (_, read_only, write) in selected {
            for mode in [read_only, write] {
                let lower = mode.to_ascii_lowercase();
                for word in ["full", "bypass", "yolo", "auto", "accept", "agent", "plan"] {
                    assert!(!lower.contains(word), "{mode} is not an asking mode");
                }
            }
        }
    }

    /// The residual names what stays unbounded, and the no-write case says the project is
    /// still writable — the boundary a reader would otherwise believe.
    #[test]
    fn the_residual_states_what_permits_do_not_bound() {
        let codex = HarnessBoundary::CODEX_ACP;
        assert_eq!(
            codex.check_advice(false),
            "an `agent:` task on codex runs in mode `read-only`: Nika judges each approval it \
             asks (journaled permit_checked), but without asking it reads any file; writes the \
             project, /tmp, $TMPDIR and its own ~/.codex (the workflow grants no fs.write); runs \
             any program in its own sandbox, its tools through a JavaScript code mode and its \
             MCP tools marked readOnlyHint (remote ones included); fetches pages with its hosted \
             web tool, whose content reaches the model; starts every configured MCP server and \
             applies your global ~/.codex/AGENTS.md and Codex plugins — permits: does not bound \
             that (`nika explain harness`). An image it generates reaches the workflow through \
             `images: { save: <path> }`: Nika writes the bytes this session sent, never a file \
             the harness reports or a copy from its shared images folder."
        );
        assert!(codex.check_advice(true).contains(
            "writes anywhere in the project, /tmp, $TMPDIR and its own ~/.codex (not only the \
             fs.write paths)"
        ));
        let witness = codex.unasked(false);
        assert!(
            witness.starts_with("Without asking, the harness reads any file; "),
            "{witness}"
        );
        assert!(witness.ends_with(HARNESS_SHARED_RESIDUAL), "{witness}");
        let claude = HarnessBoundary::CLAUDE_AGENT_ACP.check_advice(true);
        assert!(claude.contains("in mode `default`"), "{claude}");
        assert!(
            !claude.contains("writes"),
            "every Claude Code edit asks: {claude}"
        );
        assert!(
            claude.contains("fetches documentation hosts Claude Code pre-approves"),
            "{claude}"
        );
        // The Claude Code seat is opened without the user's own configuration: the row says
        // what still applies, and the shared residual speaks only of settings a harness loads.
        assert!(
            claude.contains(
                "; applies a managed Claude Code policy when the machine has one, and runs a \
                 task that names no model on the one your Claude settings name (your own \
                 permission rules, allow, ask or deny, hooks, MCP servers, plugins, skills and \
                 memory files are not loaded into the session) — permits: does not bound that"
            ),
            "{claude}"
        );
        assert!(
            HARNESS_SHARED_RESIDUAL.ends_with(
                "Settings the harness loads on its own (its permission rules, hooks and MCP \
                 servers) still apply: what they allow never asks."
            ),
            "{HARNESS_SHARED_RESIDUAL}"
        );
        // NIK-274 · only the mode that runs ordinary commands sandboxed unasked turns every
        // command ask into an escalation.
        let escalating: alloc::vec::Vec<&str> = HarnessBoundary::MEASURED
            .iter()
            .filter(|row| row.asked_command_leaves_sandbox)
            .map(|row| row.seat)
            .collect();
        assert_eq!(escalating, ["codex"]);
        let unmeasured = unmeasured_check_advice("kimi-code");
        assert!(
            unmeasured.contains("on kimi-code runs in a mode Nika has not measured"),
            "{unmeasured}"
        );
        // B-D18 · the one way an image reaches the workflow, taught only where the client
        // recognises the adapter's image tool (Codex): elsewhere `images:` receives nothing.
        assert!(codex.check_advice(true).ends_with(HARNESS_IMAGES_ADVICE));
        let gemini = HarnessBoundary::GEMINI_CLI.check_advice(true);
        for advice in [claude, gemini, unmeasured] {
            assert!(!advice.contains("images:"), "{advice}");
        }
    }

    /// Every row names, as data, the releases its display string names, and only the two
    /// rows whose adapters serve no usable legacy model setter select models through the
    /// option alone.
    #[test]
    fn every_row_holds_the_releases_it_was_measured_on() {
        let releases: alloc::vec::Vec<(&str, &[AdapterRelease])> = HarnessBoundary::MEASURED
            .iter()
            .map(|row| (row.seat, row.releases))
            .collect();
        assert_eq!(
            releases,
            [
                ("gemini-cli", &[AdapterRelease::new(0, 62, 0)][..]),
                ("codex", &[AdapterRelease::new(1, 13, 1)][..]),
                ("claude-code", &[AdapterRelease::new(0, 81, 1)][..]),
            ]
        );
        for row in HarnessBoundary::MEASURED {
            for release in row.releases {
                let shown = alloc::format!(" {release}");
                assert!(row.measured_on.contains(&shown), "{}", row.measured_on);
            }
        }
        let option_only: alloc::vec::Vec<&str> = HarnessBoundary::MEASURED
            .iter()
            .filter(|row| row.models_through_option)
            .map(|row| row.seat)
            .collect();
        assert_eq!(option_only, ["codex", "claude-code"]);
    }

    /// The lookup answers measured, carried or none from the reported name and release: a
    /// release the client cannot read, or none at all, is carried, never measured.
    #[test]
    fn a_session_is_measured_only_on_a_release_its_row_was_made_on() {
        let claude = "@agentclientprotocol/claude-agent-acp";
        let codex = "@agentclientprotocol/codex-acp";
        let measured = |name: &str, release: &str| {
            HarnessBoundary::for_release(name, Some(release)).map(|b| b.is_measured())
        };
        assert_eq!(
            HarnessBoundary::for_release(claude, Some("0.81.1")),
            Some(SessionBoundary::Measured(HarnessBoundary::CLAUDE_AGENT_ACP))
        );
        assert_eq!(
            HarnessBoundary::for_release(claude, Some("0.112.0")),
            Some(SessionBoundary::Carried {
                row: HarnessBoundary::CLAUDE_AGENT_ACP,
                reported: Some("0.112.0".into()),
            })
        );
        assert_eq!(measured(codex, "1.13.1"), Some(true));
        assert_eq!(measured("gemini-cli", "0.62.0"), Some(true));
        for (name, release) in [
            (codex, "1.12.0"),
            (codex, "1.13.10"),
            ("codex-acp", "0.16.0"),
            (claude, "0.81.0"),
            (claude, "v0.81.1"),
            (claude, "0.81.1-beta.1"),
            (claude, "nightly"),
            (claude, ""),
            ("gemini-cli", "0.62.1"),
        ] {
            assert_eq!(measured(name, release), Some(false), "{name} {release:?}");
        }
        assert_eq!(
            HarnessBoundary::for_release(codex, None),
            Some(SessionBoundary::Carried {
                row: HarnessBoundary::CODEX_ACP,
                reported: None,
            })
        );
        for unmeasured in ["Codex", "Some Agent", "@zed-industries/claude-agent-acp"] {
            assert_eq!(
                HarnessBoundary::for_release(unmeasured, Some("0.81.1")),
                None
            );
        }
    }

    /// A measured session publishes the row's text as written; a carried one keeps the same
    /// row (its modes and its stricter readings) and adds one sentence naming both releases.
    #[test]
    fn a_carried_session_names_the_release_measured_and_the_one_it_runs() {
        let row = HarnessBoundary::CLAUDE_AGENT_ACP;
        let measured = SessionBoundary::Measured(row);
        for writes in [false, true] {
            assert_eq!(measured.unasked(writes), row.unasked(writes));
        }
        let carried = HarnessBoundary::for_release(row.agents[0], Some("0.112.0"));
        let carried = carried.expect("a row for the name");
        assert_eq!(carried.row(), row, "the rule is the row's");
        let note = "What the harness does without asking was measured on claude-agent-acp \
                    0.81.1; this session's adapter reports release `0.112.0`, on which it was \
                    not measured.";
        assert_eq!(row.carried_note(Some("0.112.0")), note);
        assert_eq!(carried.note().as_deref(), Some(note));
        assert_eq!(measured.note(), None);
        assert_eq!(
            carried.unasked(true),
            alloc::format!("{} {note}", row.unasked(true))
        );
        let codex = HarnessBoundary::CODEX_ACP;
        let older = HarnessBoundary::for_release(codex.agents[0], Some("1.12.0"));
        let older = older.expect("a row for the name").row();
        assert!(
            older.asked_command_leaves_sandbox,
            "the stricter reading stays"
        );
        assert_eq!(older.mode_for_writes(true), "read-only");
        assert_eq!(
            codex.carried_note(None),
            "What the harness does without asking was measured on codex-acp 1.13.1; this \
             session's adapter reports no release, so it was not measured on the one it runs."
        );
        assert_eq!(
            codex.carried_note(Some("2.0\u{1b}[2J")),
            "What the harness does without asking was measured on codex-acp 1.13.1; this \
             session's adapter reports release `2.0?[2J`, on which it was not measured."
        );
        // A backtick cannot close the code span the release is printed in.
        assert_eq!(
            codex.carried_note(Some("1.12.0` and more")),
            "What the harness does without asking was measured on codex-acp 1.13.1; this \
             session's adapter reports release `1.12.0? and more`, on which it was not \
             measured."
        );
    }

    /// An empty or blank release is no release: the session is carried with none, and the
    /// sentence says the adapter reports none.
    #[test]
    fn an_empty_release_is_no_release() {
        let codex = HarnessBoundary::CODEX_ACP;
        let none = "What the harness does without asking was measured on codex-acp 1.13.1; this \
                    session's adapter reports no release, so it was not measured on the one it \
                    runs.";
        for blank in ["", " ", "\t \n"] {
            assert_eq!(codex.carried_note(Some(blank)), none, "{blank:?}");
            assert_eq!(
                HarnessBoundary::for_release(codex.agents[0], Some(blank)),
                Some(SessionBoundary::Carried {
                    row: codex,
                    reported: None,
                }),
                "{blank:?}"
            );
        }
    }
}
