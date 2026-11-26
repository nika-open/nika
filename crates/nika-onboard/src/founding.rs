// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `nika init [dir]` — found a repo for Nika workflows.
//!
//! Two doors, one law. **Bare on a terminal** the founding wizard runs
//! (`wizard.rs` — recipe · model · canvas · agents · scaffold · proof ·
//! the ready panel). **Any flag, `--yes`, a pipe, or CI** is the
//! scriptable twin: `--recipe` scaffolds a workflow set, `--theme`
//! stamps the VS Code DAG skin, `--wire` connects agent clients — and
//! plain `--yes` emits file receipts and a first-workflow hand-off.
//!
//! The human keeps the hand everywhere: an existing file is SKIPPED,
//! never clobbered — `--force` is the explicit override (same law as
//! `nika compile`). Every write stays inside the project (`crate::confine`:
//! a link is never written through, `--force` included). A write failure,
//! a refused write (a link, a folder leading outside) and a refused hook
//! wiring are the environment error (`exit 3`).
//! The one append-maybe surface is `.gitignore` (`crate::gitignore` —
//! adds-only: the trace-cover section joins an existing file, never a
//! rewrite, and a second run adds nothing).

use std::fmt::Write as _;
use std::path::Path;

use crate::recipes::{self, ScaffoldStatus};
use crate::{Audit, Outcome, Wire, briefs, codes, confine, gitignore, hook_copies, project_file};

pub use briefs::agents_md;

/// The `--recipe` vocabulary for clap (`value_parser`) — pinned against
/// the register by test so the two can never drift.
pub const RECIPE_NAMES: [&str; 5] = ["agentic", "starter", "ship", "content", "minimal"];

/// The workflow a plain `nika init` founds around (#1283): the hello
/// lesson `nika try` rehearses — one file that audits clean on this
/// binary and runs offline under `--model mock/echo`. `--recipe minimal`
/// is the explicit no-workflow door.
pub const DEFAULT_EXAMPLE: &str = "01-hello";

/// The `--theme` vocabulary — `nika.dag.theme`'s own enum (the VS Code
/// extension's canvas skin), stamped into `.vscode/settings.json`. The
/// composition root hands the clap word to [`CanvasTheme::parse`]; here
/// it stays plain (no CLI-framework dependency below the root).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasTheme {
    /// The brand skin — engineered black · verb hues.
    Nika,
    /// Adaptive — follows the editor's colors.
    Editor,
    /// Terminal green.
    Phosphor,
    /// Let the extension decide.
    Auto,
}

/// The `--theme` vocabulary for clap (`value_parser`), in [`CanvasTheme`]
/// order — pinned against [`CanvasTheme::parse`] by test.
pub const CANVAS_THEMES: [&str; 4] = ["nika", "editor", "phosphor", "auto"];

impl CanvasTheme {
    /// The wire word `nika.dag.theme` speaks.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nika => "nika",
            Self::Editor => "editor",
            Self::Phosphor => "phosphor",
            Self::Auto => "auto",
        }
    }

    /// The wire word back to the skin (`--theme` arrives as a clap string
    /// at the root · the wizard's menu resolves its own way).
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [Self::Nika, Self::Editor, Self::Phosphor, Self::Auto]
            .into_iter()
            .find(|c| c.as_str() == word)
    }
}

/// What `init` does (or declines to do) for one target file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Write `body` to `path` (it is absent, or `--force`).
    Create { path: String, body: &'static str },
    /// Leave `path` untouched — it already exists (`--force` to overwrite).
    Skip { path: String },
}

impl Action {
    /// The joined target path, whatever the decision.
    fn path(&self) -> &str {
        match self {
            Self::Create { path, .. } | Self::Skip { path } => path,
        }
    }
}

/// PURE plan over an injected existence oracle — `Create` an absent file (or
/// any when `force`), `Skip` an existing one. Testable without the filesystem.
pub(crate) fn plan(dir: &str, exists: &dyn Fn(&str) -> bool, force: bool) -> Vec<Action> {
    briefs::targets()
        .into_iter()
        .map(|(rel, body)| {
            let path = join(dir, rel);
            if !force && exists(&path) {
                Action::Skip { path }
            } else {
                Action::Create { path, body }
            }
        })
        .collect()
}

/// Join a base dir and a relative path the same way the apply step will.
fn join(dir: &str, rel: &str) -> String {
    Path::new(dir).join(rel).to_string_lossy().into_owned()
}

/// Render the report (✔ created · · skipped · ✖ write error) — the
/// familiar row prefixes; the wizard has
/// its own themed register.
pub(crate) fn render(lines: &[(char, String)]) -> String {
    let mut s = String::new();
    for (glyph, msg) in lines {
        let _ = writeln!(s, "{glyph} {msg}");
    }
    s
}

/// The beginner's next move · init used to end SILENTLY (the 2026-07-05
/// beginner walk: « you init and… sit there ») — an onboarding surface
/// must hand over to the next command. Golden path: offline proof in
/// 10s → scaffold → audit-before-tokens. The shape scripts have seen
/// since #158, now the hand-off of the workflow-less founds only
/// (`--recipe minimal` · a headless `starter`): a plain init founds
/// around [`DEFAULT_EXAMPLE`] and hands over to THAT file (#1283).
pub(crate) const NEXT_BLOCK: &str = "next ·\n  nika try 01-hello   # offline rehearsal\n  nika compile hello hello.nika   # explicit first file · mock/echo\n  nika compile --list   # exact skeletons; missing answers remain questions\n  nika check hello.nika   # audit before run";

/// The scriptable path — briefs report with purposes, then each
/// flagged extra as its own receipt block, then the hand-off. The door
/// logic (bare-TTY → the wizard) lives at the composition root; this
/// crate exposes the two paths and the root routes.
#[must_use]
pub fn scripted_run(
    dir: &str,
    force: bool,
    recipe: Option<&str>,
    example: Option<&str>,
    canvas: Option<CanvasTheme>,
    wires: &[&str],
    audit: &Audit<'_>,
    wire: &Wire<'_>,
) -> Outcome {
    // One founding source at a time — the clap surface already refuses
    // the pair; a direct lib call gets the same honesty.
    if recipe.is_some() && example.is_some() {
        return Outcome {
            text: "pass --recipe OR --example, not both — one founding source".to_owned(),
            code: codes::ENV,
        };
    }
    let rows = apply_briefs(dir, force, canvas);
    // The trace cover and the project file ride the same report —
    // adds-only (`gitignore.rs` · `project_file.rs`), so a re-run or the
    // human's own entry is a calm skip row, and a write failure is the
    // same exit-3 class as a brief's.
    let git = gitignore::ensure(dir);
    let project = project_file::ensure(dir, force);
    // A refused hook wiring stops the founding like a write failure (every
    // other brief, the trace cover and the project file are written).
    let failed = rows
        .iter()
        .any(|(_, o)| matches!(o, BriefOutcome::Failed(_) | BriefOutcome::Refused(..)))
        || matches!(git.1, gitignore::Outcome::Failed(_))
        || matches!(project.1, project_file::Outcome::Failed(_));
    let mut lines = brief_rows(dir, &rows);
    lines.push(gitignore::report(&git.0, &git.1));
    lines.push(project_file::report(&project.0, &project.1));
    let mut text = render(&lines);
    if failed {
        return Outcome::env(text);
    }

    let (first_workflow, worst, has_drafts) =
        match found_from_source(dir, force, recipe, example, audit, &mut text) {
            Ok(pair) => pair,
            Err(out) => return out,
        };

    // `starter`'s workflow step IS the three-question conversation, so its
    // template set is empty by design (`recipes::RECIPES`) and a scripted
    // run scaffolds none. Measured 2026-08-03: `--recipe starter -y` then
    // lands byte-identical to `--recipe minimal`, output included — a flag
    // that silently becomes another flag. The wiring did land; say which
    // half a pipe cannot deliver, and where the twin lives.
    if recipe == Some("starter") && first_workflow.is_none() {
        let _ = writeln!(
            text,
            "· starter's workflow step is a conversation — a script cannot answer it. \
             The project files were scaffolded; run bare `nika init` on a terminal for the questions, \
             or `nika compile hello hello.nika` for an explicit first file."
        );
    }

    let wiring = wire_receipts(dir, wires, wire);
    text.push_str(&wiring.text);
    text.push_str("\nteam ·\n  nika.yaml                        # shared cost ceiling + trace retention · commented until your team edits it\n  Git: commit reviewed workflows, goldens, guides, nika.yaml and shared settings.\n  Keep credentials, signing private keys and raw traces local; review artifacts.\n  Read NIKA.md for the file map and first workflow.\n");

    let next = first_workflow.map_or_else(
        || NEXT_BLOCK.to_owned(),
        |first| {
            if has_drafts {
                format!(
                    "next ·\n  $EDITOR {first}                   # fill the remaining `<SLOT: …>` values\n  nika check {first}                    # audit before a single token\n  nika run {first} --model mock/echo   # mocked envelope inference; task model pins, tools and effects remain real\n  nika explain <NIKA-XXXX>              # every finding teaches"
                )
            } else {
                format!(
                "next ·\n  nika check {first}                    # audit before a single token\n  nika run {first} --model mock/echo   # mocked envelope inference; task model pins, tools and effects remain real\n  nika explain <NIKA-XXXX>              # every finding teaches"
                )
            }
        },
    );
    Outcome {
        text: format!("{text}\n{next}"),
        code: worst.max(wiring.code),
    }
}

/// The scripted register's brief rows: joined paths and the receipt
/// prefixes scripts parse; each newly created brief says what it is for, so
/// the transcript helps humans review the generated setup.
fn brief_rows(dir: &str, rows: &[(String, BriefOutcome)]) -> Vec<(char, String)> {
    rows.iter()
        .map(|(path, outcome)| match outcome {
            BriefOutcome::Created => (
                '✔',
                format!("created {path} — {}", briefs::purpose(&rel_to(dir, path))),
            ),
            BriefOutcome::Skipped => (
                '·',
                format!("skipped {path} (exists · --force to overwrite)"),
            ),
            BriefOutcome::KeptLink => ('·', format!("skipped {path} ({KEPT_LINK})")),
            BriefOutcome::KeptAside(note) => ('·', format!("skipped {path} ({note})")),
            BriefOutcome::KeptWiring(scripts, kept) => {
                ('·', kept_wiring_line(path, scripts, *kept))
            }
            BriefOutcome::Refused(scripts, why) => ('✖', refused_line(path, scripts, *why)),
            BriefOutcome::Failed(e) => ('✖', format!("{path}: {e}")),
        })
        .collect()
}

/// Resolve the ONE founding source into a scaffold set, speak its
/// report, and hand back the first workflow + the worst audit code.
///
/// The example lane (a verbatim lesson) and a recipe (a template set)
/// are two doors to the same ladder — the report and proof below are
/// byte-identical between them. Neither door taken (plain `--yes`)
/// founds around [`DEFAULT_EXAMPLE`] (#1283 · a project with zero
/// workflows taught nothing); `--recipe minimal` is the briefs-only door.
///
/// `Err` carries the honest refusal an unknown recipe earns on a direct
/// lib call — clap's `value_parser` guards the CLI door, not this one.
fn found_from_source(
    dir: &str,
    force: bool,
    recipe: Option<&str>,
    example: Option<&str>,
    audit: &Audit<'_>,
    text: &mut String,
) -> Result<(Option<String>, u8, bool), Outcome> {
    let scaffolded = match (example, recipe) {
        (Some(slug), _) => recipes::scaffold_example(dir, slug, force),
        (None, Some(name)) => {
            let Some(r) = recipes::recipe(name) else {
                return Err(Outcome {
                    text: format!(
                        "unknown recipe `{name}` — the register: {}",
                        RECIPE_NAMES.join(" · ")
                    ),
                    code: codes::FILE,
                });
            };
            recipes::scaffold(dir, r, None, force)
        }
        (None, None) => recipes::scaffold_example(dir, DEFAULT_EXAMPLE, force),
    };
    scaffold_report(dir, &scaffolded, audit, text)
}

/// Speak one scaffold set's report bytes into `text` (✔/·/✖ rows —
/// the shape scripts parse) and run the proof ladder over what was
/// created. Shared verbatim by the recipe and example lanes.
fn scaffold_report(
    dir: &str,
    scaffolded: &[(String, ScaffoldStatus)],
    audit: &Audit<'_>,
    text: &mut String,
) -> Result<(Option<String>, u8, bool), Outcome> {
    let mut created: Vec<String> = Vec::new();
    for (path, status) in scaffolded {
        let rel = rel_to(dir, path);
        match status {
            ScaffoldStatus::Created => {
                let _ = writeln!(text, "✔ created {rel} — {}", briefs::purpose(&rel));
                // The proof ladder audits WORKFLOWS — the generated
                // index rides the report but never the check.
                if nika_source::is_canonical_program_path(path) {
                    created.push(path.clone());
                }
            }
            ScaffoldStatus::Skipped => {
                let _ = writeln!(text, "· skipped {rel} (exists · --force to overwrite)");
            }
            ScaffoldStatus::Failed(e) => {
                return Err(Outcome::env(format!("{text}✖ {rel}: {e}\n")));
            }
        }
    }
    let first = created.first().map(|p| rel_to(dir, p));
    let mut worst = codes::OK;
    let mut has_drafts = false;
    for receipt in proof_receipts(dir, &created, audit) {
        worst = worst.max(receipt.code);
        has_drafts |= receipt.draft;
        let _ = writeln!(text, "{}", receipt.line);
    }
    Ok((first, worst, has_drafts))
}

/// What one brief write came to — the registers compose their own
/// message shapes over it (scripted keeps the historical joined-path
/// bytes · the wizard rail speaks project-relative).
pub(crate) enum BriefOutcome {
    Created,
    Skipped,
    /// Left as it is: a link to a file inside the project (`CLAUDE.md`
    /// pointing at `AGENTS.md`), which init never writes through,
    /// `--force` included.
    KeptLink,
    /// Left as it is, never followed or read: at a name init would only
    /// skip stands a link leading out (or to nothing), a folder leading
    /// out, or something not a regular file. No write is needed, so
    /// nothing is refused; the note says which ([`confine::Refusal::kept`]).
    KeptAside(&'static str),
    /// An existing hook-wiring file (or a link to one inside the project),
    /// kept as it is, beside scripts that do not hold what `init` writes for
    /// them (joined paths): the row says so, since that file's hooks may run
    /// them.
    KeptWiring(Vec<String>, Kept),
    /// A hook-wiring file left unwritten: its hooks would run these scripts
    /// (joined paths), which do not hold what `init` writes for them.
    Refused(Vec<String>, Blocker),
    /// A write that failed or was refused (`crate::confine`); the text is
    /// the cause and its remedy.
    Failed(String),
}

/// The note of a [`BriefOutcome::KeptLink`] row, both registers.
pub(crate) const KEPT_LINK: &str = "a link · init never writes through a link";

/// What a kept hook wiring is: the note of its row differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kept {
    /// A regular file: `--force` rewrites it.
    File,
    /// A link to a file inside the project, never written through.
    Link,
    /// Kept aside, never followed (the note of [`BriefOutcome::KeptAside`]).
    Aside(&'static str),
}

/// Why a hook script blocks its wiring, worst last: the remedy differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Blocker {
    /// A regular file with other bytes or without its exec bit: the
    /// repository's own, or an older init's copy. `--force` rewrites it.
    Foreign,
    /// A link, kept: `--force` never writes through it.
    Link,
    /// Its own write failed or was refused in this run (its row says why).
    Unwritten,
}

/// The report row of an existing hook wiring kept beside scripts init did
/// not write, shared by both registers (each passes its own path spelling).
/// Information only: the file is the repository's, and so are the scripts.
pub(crate) fn kept_wiring_line(wiring: &str, scripts: &[String], kept: Kept) -> String {
    let them = if scripts.len() == 1 { "it" } else { "them" };
    let note = match kept {
        Kept::File => "exists · --force to overwrite",
        Kept::Link => KEPT_LINK,
        Kept::Aside(note) => note,
    };
    format!(
        "skipped {wiring} ({note}) · its hooks may run {}, which init did not write: review {them} before trusting it",
        scripts.join(", ")
    )
}

/// The report row of a refused hook wiring, shared by both registers (each
/// passes its own path spelling). The remedy leads with the step that
/// touches nothing else; `--force` is named only where it helps, with what
/// else it rewrites.
pub(crate) fn refused_line(wiring: &str, scripts: &[String], why: Blocker) -> String {
    let (them, rows) = if scripts.len() == 1 {
        ("it", "its own row names")
    } else {
        ("them", "their own rows name")
    };
    let list = scripts.join(", ");
    match why {
        Blocker::Foreign => format!(
            "refused {wiring} — its hooks would run {list}, which init did not write · remove {them} and run nika init again, or rerun with --force, which rewrites every file init writes, AGENTS.md, CLAUDE.md, .mcp.json and settings included"
        ),
        Blocker::Link => format!(
            "refused {wiring} — its hooks would run {list}, which init did not write (a link, never written through, --force included) · remove {them} and run nika init again"
        ),
        Blocker::Unwritten => format!(
            "refused {wiring} — its hooks would run {list}, which init could not write · fix what {rows}, then run nika init again"
        ),
    }
}

/// Write the briefs per `plan`, honoring the canvas stamp on a CREATED
/// settings file and the project-copy stamps on CREATED hook scripts.
/// Skipped files retain their content and provenance.
///
/// A hook-wiring file is written last, and only when every script its hooks
/// run is a regular file holding exactly what `init` writes for it: written
/// by this run, or already identical. A skipped script with other bytes or
/// without its exec bit (a repository's own, or an older init's copy)
/// refuses the wiring instead, so
/// `init` never points a client at a file it did not write. `--force`
/// rewrites the scripts first, which makes the wiring safe again; a script
/// that is a link stays a blocker, since no write goes through a link.
///
/// Anything at a target counts as there (a dangling link included), so
/// nothing is created through a link; every write goes through
/// `crate::confine`.
pub(crate) fn apply_briefs(
    dir: &str,
    force: bool,
    canvas: Option<CanvasTheme>,
) -> Vec<(String, BriefOutcome)> {
    let wiring = |path: &str| {
        hook_copies::HOOK_WIRING
            .iter()
            .find(|(rel, _)| join(dir, rel) == path)
            .map(|(_, scripts)| scripts)
    };
    let there = |p: &str| std::fs::symlink_metadata(p).is_ok();
    let (hooks, rest): (Vec<_>, Vec<_>) = plan(dir, &there, force)
        .into_iter()
        .enumerate()
        .partition(|(_, action)| wiring(action.path()).is_some());
    let mut rows: Vec<(usize, String, BriefOutcome)> = Vec::new();
    for (at, action) in rest {
        let (path, outcome) = apply(dir, action, canvas);
        rows.push((at, path, outcome));
    }
    for (at, action) in hooks {
        let scripts = wiring(action.path()).map_or(&[][..], |s| &s[..]);
        let (path, outcome) = match (action, blockers(dir, scripts, &rows)) {
            // The wiring's own refusal (a link, a folder leading out) comes
            // first: it names what to fix at that path.
            (Action::Create { path, .. }, Some((blocked, why))) => {
                match confine::occupied(Path::new(dir), Path::new(&path)) {
                    Err(e) => (path, BriefOutcome::Failed(e.to_string())),
                    // A link inside the project is never written (`--force`
                    // included), so nothing is refused: it is kept, and named.
                    Ok(_) if confine::link_inside(Path::new(dir), Path::new(&path)) => {
                        (path, BriefOutcome::KeptWiring(blocked, Kept::Link))
                    }
                    Ok(_) => (path, BriefOutcome::Refused(blocked, why)),
                }
            }
            (action, blocked) => match (apply(dir, action, canvas), blocked) {
                ((path, BriefOutcome::Skipped), Some((scripts, _))) => {
                    (path, BriefOutcome::KeptWiring(scripts, Kept::File))
                }
                ((path, BriefOutcome::KeptLink), Some((scripts, _))) => {
                    (path, BriefOutcome::KeptWiring(scripts, Kept::Link))
                }
                ((path, BriefOutcome::KeptAside(note)), Some((scripts, _))) => {
                    (path, BriefOutcome::KeptWiring(scripts, Kept::Aside(note)))
                }
                (row, _) => row,
            },
        };
        rows.push((at, path, outcome));
    }
    rows.sort_by_key(|(at, ..)| *at);
    rows.into_iter()
        .map(|(_, path, outcome)| (path, outcome))
        .collect()
}

/// The scripts of one wiring that do not hold what `init` writes for them
/// (joined paths), with the worst reason, or `None` when every one does.
/// `rows` are this run's outcomes so far: a script whose own write failed
/// is named as such.
fn blockers(
    dir: &str,
    scripts: &[&str],
    rows: &[(usize, String, BriefOutcome)],
) -> Option<(Vec<String>, Blocker)> {
    let mut worst = None;
    let mut blocked = Vec::new();
    for rel in scripts.iter().filter(|rel| !holds_scaffold(dir, rel)) {
        let path = join(dir, rel);
        let failed = rows
            .iter()
            .any(|(_, p, o)| *p == path && matches!(o, BriefOutcome::Failed(_)));
        let why = if failed {
            Blocker::Unwritten
        } else if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            Blocker::Link
        } else {
            Blocker::Foreign
        };
        worst = worst.max(Some(why));
        blocked.push(path);
    }
    worst.map(|why| (blocked, why))
}

/// Carry out one planned brief.
fn apply(dir: &str, action: Action, canvas: Option<CanvasTheme>) -> (String, BriefOutcome) {
    match action {
        Action::Skip { path } => {
            let target = Path::new(&path);
            let outcome = match confine::occupied(Path::new(dir), target) {
                // Nothing is written at a skipped name: what stands there is
                // kept, never followed, and the founding goes on.
                Err(e) => confine::refusal(&e).map_or_else(
                    || BriefOutcome::Failed(e.to_string()),
                    |why| BriefOutcome::KeptAside(why.kept()),
                ),
                Ok(_)
                    if std::fs::symlink_metadata(target)
                        .is_ok_and(|m| m.file_type().is_symlink()) =>
                {
                    BriefOutcome::KeptLink
                }
                Ok(_) => BriefOutcome::Skipped,
            };
            (path, outcome)
        }
        // `--force` over a link inside the project: kept, never written
        // through (the file it leads to is the project's own row).
        Action::Create { path, .. } if confine::link_inside(Path::new(dir), Path::new(&path)) => {
            (path, BriefOutcome::KeptLink)
        }
        Action::Create { path, body } => {
            let themed;
            let stamped = hook_copies::scaffold_body(&path, body);
            let body = match canvas {
                Some(c) if path.ends_with(".vscode/settings.json") => {
                    themed = themed_settings(c);
                    themed.as_str()
                }
                _ => &stamped,
            };
            let outcome = match confine::write(Path::new(dir), Path::new(&path), body.as_bytes()) {
                Ok(()) => BriefOutcome::Created,
                Err(e) => BriefOutcome::Failed(e.to_string()),
            };
            (path, outcome)
        }
    }
}

/// Whether the scaffold target `rel` under `dir` is a regular file inside
/// the project (not a link, no folder on the way leading out) holding
/// exactly the bytes `init` writes for it, stamps included, and, for a
/// shell script, executable as init leaves it (a copy a host cannot start
/// is not init's). The kind and
/// the size are read first, so a link, a FIFO or a file of another length
/// is refused without being opened; the read itself never waits and stops
/// at that size (`confine::read_text`), so a FIFO swapped in meanwhile is
/// refused too.
fn holds_scaffold(dir: &str, rel: &str) -> bool {
    let Some((_, body)) = briefs::targets().into_iter().find(|(r, _)| *r == rel) else {
        return false;
    };
    let expected = hook_copies::scaffold_body(rel, body);
    let path = join(dir, rel);
    let (root, at) = (Path::new(dir), Path::new(&path));
    let Ok(size) = u64::try_from(expected.len()) else {
        return false;
    };
    confine::plain_file(root, at)
        && std::fs::symlink_metadata(at).is_ok_and(|m| m.len() == size && runnable(rel, &m))
        && confine::read_text(root, at, size).is_ok_and(|text| text.as_deref() == Some(&*expected))
}

/// Whether a scaffold file of that name has the mode init gives it: a
/// shell script carries its exec bits (init writes `0755`); any other file
/// passes.
fn runnable(rel: &str, meta: &std::fs::Metadata) -> bool {
    let script = Path::new(rel)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("sh"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        !script || meta.permissions().mode() & 0o111 == 0o111
    }
    #[cfg(not(unix))]
    {
        let _ = (script, meta);
        true
    }
}

/// The schema-wiring settings body with `nika.dag.theme` stamped in —
/// parsed and re-emitted (never string-spliced), so the wiring survives
/// any future shape of the const.
fn themed_settings(canvas: CanvasTheme) -> String {
    let mut value: serde_json::Value =
        serde_json::from_str(briefs::vscode_settings()).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "nika.dag.theme".to_owned(),
            serde_json::Value::String(canvas.as_str().to_owned()),
        );
    }
    let mut body = serde_json::to_string_pretty(&value).unwrap_or_default();
    body.push('\n');
    body
}

/// Audit every scaffolded workflow NOW — the ladder inside the first
/// minute is the product's argument. Clean collapses to one receipt
/// line; findings expand to the full report (the vitest law: collapse
/// success, expand failure).
pub(crate) struct ProofReceipt {
    pub(crate) line: String,
    pub(crate) code: u8,
    pub(crate) draft: bool,
}

pub(crate) fn proof_receipts(
    dir: &str,
    created: &[impl AsRef<str>],
    audit: &Audit<'_>,
) -> Vec<ProofReceipt> {
    created
        .iter()
        .map(|path| {
            let path = path.as_ref();
            let audit = audit(path);
            let rel = rel_to(dir, path);
            if audit.code == codes::OK {
                let tail = audit
                    .text
                    .lines()
                    .rev()
                    .find(|l| l.contains("audited") || l.contains("not a workflow yet"))
                    .map_or_else(|| "audited clean".to_owned(), |l| l.trim().to_owned());
                ProofReceipt {
                    draft: tail.contains("not a workflow yet"),
                    line: format!("  {tail} ← {rel}"),
                    code: codes::OK,
                }
            } else {
                ProofReceipt {
                    line: format!("{}\n✖ {rel} — findings above", audit.text.trim_end()),
                    code: audit.code,
                    draft: false,
                }
            }
        })
        .collect()
}

/// Connect the picked agent clients through the REAL `wire` verb —
/// each client's own receipt, indented under one header.
pub(crate) fn wire_receipts(dir: &str, wires: &[&str], wire: &Wire<'_>) -> Outcome {
    if wires.is_empty() {
        return Outcome {
            text: String::new(),
            code: codes::OK,
        };
    }
    let mut text = "wiring ·\n".to_owned();
    let mut code = codes::OK;
    for client in wires {
        let out = wire(client, dir);
        code = code.max(out.code);
        for l in out.text.lines() {
            let _ = writeln!(text, "  {l}");
        }
    }
    Outcome { text, code }
}

/// A path relative to the project dir when it nests there.
fn rel_to(dir: &str, path: &str) -> String {
    Path::new(path)
        .strip_prefix(dir)
        .map_or_else(|_| path.to_owned(), |p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
#[cfg(unix)]
mod boundary_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use nika_display::theme::Theme;

    const PLAIN: Theme = Theme::new(false, false, false);

    fn stub_audit(path: &str) -> Outcome {
        Outcome::ok(format!("  ✔ audited (stub) ← {path}"))
    }
    fn stub_wire(client: &str, _dir: &str) -> Outcome {
        Outcome::ok(format!("{client}: wired (stub)"))
    }

    /// The clap vocabulary and the enum cannot drift: every word parses
    /// to the skin that speaks it, and nothing else parses.
    #[test]
    fn canvas_theme_words_round_trip_through_the_register() {
        for word in CANVAS_THEMES {
            assert_eq!(
                CanvasTheme::parse(word).map(CanvasTheme::as_str),
                Some(word)
            );
        }
        assert_eq!(CanvasTheme::parse("neon"), None);
    }

    #[test]
    fn scripted_init_preserves_a_wire_refusal() {
        let dir =
            std::env::temp_dir().join(format!("nika-init-wire-refusal-{}", std::process::id()));
        let out = scripted_run(
            dir.to_str().expect("path"),
            false,
            None,
            None,
            None,
            &["all"],
            &stub_audit,
            &|_, _| Outcome::env("wiring requires a decision".to_owned()),
        );
        assert_eq!(out.code, codes::ENV, "{}", out.text);
        assert!(out.text.contains("wiring requires a decision"));
        assert!(!out.text.contains("wired ·"));
        assert!(
            dir.join("AGENTS.md").exists(),
            "completed scaffold is retained"
        );
        std::fs::remove_dir_all(dir).expect("remove owned fixture");
    }

    /// The old 7-arg `run` shape, test-side: scripted path with stubs
    /// (the door logic lives at the composition root now).
    fn run(
        dir: &str,
        force: bool,
        _yes: bool,
        recipe: Option<&str>,
        canvas: Option<CanvasTheme>,
        wires: &[&str],
        _theme: Theme,
    ) -> Outcome {
        scripted_run(
            dir,
            force,
            recipe,
            None,
            canvas,
            wires,
            &stub_audit,
            &stub_wire,
        )
    }

    /// The example lane: `--example 01-hello` founds the project around
    /// ONE verbatim lesson — file + generated index + proof + tailored
    /// next; `--recipe` AND `--example` together refuse honestly.
    #[test]
    fn example_lane_founds_around_one_lesson() {
        let tmp = std::env::temp_dir().join(format!("nika-init-example-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let dir = tmp.to_string_lossy().into_owned();

        let out = scripted_run(
            &dir,
            false,
            None,
            Some("01-hello"),
            None,
            &[],
            &stub_audit,
            &stub_wire,
        );
        assert_eq!(out.code, codes::OK, "{}", out.text);
        assert!(
            out.text.contains("created workflows/01-hello.nika"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("created workflows/README.md"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("nika run workflows/01-hello.nika"),
            "tailored next: {}",
            out.text
        );
        let body = std::fs::read_to_string(tmp.join("workflows/01-hello.nika")).expect("written");
        assert_eq!(
            body,
            nika_pack::example("01-hello").expect("embedded"),
            "verbatim"
        );

        let both = scripted_run(
            &dir,
            false,
            Some("agentic"),
            Some("01-hello"),
            None,
            &[],
            &stub_audit,
            &stub_wire,
        );
        assert_eq!(both.code, codes::ENV);
        assert!(both.text.contains("not both"), "{}", both.text);

        let unknown = scripted_run(
            &dir,
            true,
            None,
            Some("nope"),
            None,
            &[],
            &stub_audit,
            &stub_wire,
        );
        assert_eq!(unknown.code, codes::ENV, "{}", unknown.text);
        assert!(unknown.text.contains("nika try"), "{}", unknown.text);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn effectful_scaffolds_teach_check_before_run_without_an_offline_promise() {
        for draft in [false, true] {
            let tmp = std::env::temp_dir().join(format!(
                "nika-init-effectful-{}-{draft}",
                std::process::id()
            ));
            let audit = |_: &str| {
                Outcome::ok(
                    if draft {
                        "not a workflow yet"
                    } else {
                        "audited clean"
                    }
                    .to_owned(),
                )
            };
            let out = scripted_run(
                tmp.to_str().expect("path"),
                false,
                None,
                Some("03-exec-pipeline"),
                None,
                &[],
                &audit,
                &stub_wire,
            );
            let source = std::fs::read_to_string(tmp.join("workflows/03-exec-pipeline.nika"))
                .expect("effectful example");
            assert!(
                source.contains("exec:"),
                "the fixture has real subprocess effects"
            );
            let next = out.text.split("next ·").last().expect("handover");
            let check = next.find("nika check ").expect("inspection command");
            let run = next.find("nika run ").expect("execution command");
            assert!(check < run, "inspection precedes real execution: {next}");
            assert!(
                !next.contains("offline proof") && !next.contains("zero keys"),
                "mock does not remove effects or task model pins: {next}"
            );
            assert!(
                next.contains("effects remain real"),
                "handover names the execution boundary: {next}"
            );
            std::fs::remove_dir_all(tmp).expect("remove temp project");
        }
    }

    #[test]
    fn successful_init_hands_over_to_the_next_command() {
        // The 2026-07-05 beginner walk: init ended SILENTLY (4 files ·
        // no workflow · no next step). An onboarding surface must hand
        // over — the ok-path text hands over to the founded workflow.
        let tmp = std::env::temp_dir().join(format!("nika-init-handover-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let out = run(
            tmp.to_str().expect("utf8"),
            false,
            true,
            None,
            None,
            &[],
            PLAIN,
        );
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(out.code, codes::OK);
        assert!(out.text.contains("next ·"), "{}", out.text);
        assert!(out.text.contains("nika check workflows/01-hello.nika"));
        assert!(
            out.text
                .contains("nika run workflows/01-hello.nika --model mock/echo")
        );
    }

    /// #1283 · a plain scripted init founds a COMPLETE project: the
    /// project file (adds-only · the skip row on a re-run) and the hello
    /// lesson as the first workflow, every created row saying why it
    /// exists; `--recipe minimal` stays the briefs-only door with the
    /// classic hand-off.
    #[test]
    fn plain_init_lays_the_project_file_and_the_hello_lesson() {
        let tmp = std::env::temp_dir().join(format!("nika-init-default-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let d = tmp.to_str().expect("utf8");
        let out = run(d, false, true, None, None, &[], PLAIN);
        assert_eq!(out.code, codes::OK, "{}", out.text);
        assert!(
            out.text
                .lines()
                .any(|l| l.starts_with("✔ created ") && l.contains("nika.yaml — team defaults")),
            "the project file row says why: {}",
            out.text
        );
        assert!(
            out.text
                .contains("✔ created workflows/01-hello.nika — a workflow"),
            "the workflow row says why: {}",
            out.text
        );
        assert!(tmp.join("nika.yaml").exists() && tmp.join("workflows/01-hello.nika").exists());
        assert!(
            !out.text.contains(NEXT_BLOCK) && !out.text.contains("--project-file"),
            "the hand-off is the founded file, the team block names the laid file: {}",
            out.text
        );
        let again = run(d, false, true, None, None, &[], PLAIN);
        assert!(
            again
                .text
                .lines()
                .any(|l| l.starts_with("· skipped ") && l.contains("nika.yaml (exists")),
            "adds-only on a re-run: {}",
            again.text
        );
        let minimal = run(d, false, true, Some("minimal"), None, &[], PLAIN);
        assert!(minimal.text.contains(NEXT_BLOCK), "{}", minimal.text);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// `--yes` (and any non-terminal) keeps the scriptable file report —
    /// the report and the classic hand-off, zero prompts.
    #[test]
    fn yes_keeps_the_non_interactive_shape() {
        let tmp = std::env::temp_dir().join(format!("nika-init-yes-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let out = run(
            tmp.to_str().expect("utf8"),
            false,
            true,
            Some("minimal"),
            None,
            &[],
            PLAIN,
        );
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(out.code, codes::OK);
        assert!(out.text.contains("✔ created"), "{}", out.text);
        assert!(
            out.text.contains(NEXT_BLOCK),
            "the classic block survives verbatim: {}",
            out.text
        );
    }

    /// The trace cover lands in the scripted lane (T1): a fresh found
    /// lays `.gitignore` with the `.nika/traces/` entry and the report
    /// says so; a second run is the calm skip row and the bytes are
    /// identical (adds-only — init never rewrites the human's file).
    #[test]
    fn scripted_init_lays_the_traces_cover_adds_only() {
        let tmp = std::env::temp_dir().join(format!("nika-init-gitignore-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let out = run(
            tmp.to_str().expect("utf8"),
            false,
            true,
            None,
            None,
            &[],
            PLAIN,
        );
        assert_eq!(out.code, codes::OK, "{}", out.text);
        assert!(
            out.text.contains("created ") && out.text.contains(".gitignore"),
            "the cover row rides the report: {}",
            out.text
        );
        let body = std::fs::read_to_string(tmp.join(".gitignore")).expect("gitignore written");
        assert!(body.contains(".nika/traces/"), "the cover entry: {body}");
        for entry in [".nika/serve.token", ".nika/serve/"] {
            assert!(
                body.lines().any(|line| line == entry),
                "the `nika serve` credential and state stay local too ({entry}): {body}"
            );
        }

        let again = run(
            tmp.to_str().expect("utf8"),
            false,
            true,
            None,
            None,
            &[],
            PLAIN,
        );
        assert_eq!(again.code, codes::OK, "{}", again.text);
        assert!(
            again
                .text
                .contains(".gitignore (.nika/traces/ already ignored)"),
            "the second run is a calm skip row: {}",
            again.text
        );
        assert_eq!(
            std::fs::read_to_string(tmp.join(".gitignore")).expect("read"),
            body,
            "adds-only: the second run changed nothing"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn fresh_project_routes_to_the_shipped_authoring_resources() {
        let tmp = std::env::temp_dir().join(format!("nika-authoring-route-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("temp project");
        let rows = apply_briefs(tmp.to_str().expect("path"), false, None);
        assert!(
            rows.iter()
                .all(|(_, outcome)| matches!(outcome, BriefOutcome::Created))
        );
        let entry = std::fs::read_to_string(tmp.join("AGENTS.md")).expect("entry");
        let skill_path = ".agents/skills/nika-authoring/SKILL.md";
        assert!(
            entry.contains(&format!("]({skill_path})")),
            "the loaded entry must route to the local skill"
        );
        let skill = std::fs::read_to_string(tmp.join(skill_path)).expect("local skill");
        let references = tmp.join(".agents/skills/nika-authoring/references");
        // A guide may be linked from more than one passage: reachability is per guide.
        let mut linked = std::collections::BTreeSet::new();
        for part in skill.split("](references/").skip(1) {
            let relative = part.split(')').next().expect("reference link");
            assert!(
                std::fs::read_to_string(references.join(relative)).is_ok(),
                "reference {relative} is readable"
            );
            linked.insert(relative.to_owned());
        }
        let shipped: std::collections::BTreeSet<String> = std::fs::read_dir(&references)
            .expect("the references are written")
            .map(|entry| entry.expect("a reference entry").file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        let guides = std::collections::BTreeSet::from(
            [
                "capabilities.md",
                "composition.md",
                "decisions-and-effects.md",
                "engine-behaviors.md",
                "job-anatomy.md",
                "language.md",
                "native-and-artifacts.md",
                "paid-inference.md",
                "project.md",
                "routing.md",
                "seats-and-images.md",
                "validation-and-execution.md",
                "workspace-and-examples.md",
            ]
            .map(str::to_owned),
        );
        assert_eq!(shipped, guides, "the thirteen guides the briefs write");
        assert_eq!(
            linked, guides,
            "all conditional guides reachable from the entry"
        );
        std::fs::remove_dir_all(tmp).expect("remove temp project");
    }

    #[test]
    fn project_hook_carries_its_generator_version_and_reruns_preserve_it() {
        let tmp = std::env::temp_dir().join(format!("nika-hook-provenance-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("temp project");
        apply_briefs(tmp.to_str().expect("path"), false, None);
        let hook = tmp.join(".cursor/hooks-nika/session-context.sh");
        let body = std::fs::read_to_string(&hook).expect("project hook");
        assert!(
            body.contains(concat!(
                "scaffold_version=\"",
                env!("CARGO_PKG_VERSION"),
                "\""
            )),
            "the relocated hook must retain its producer version"
        );
        std::fs::write(&hook, "# user-owned hook\n").expect("custom hook");
        apply_briefs(tmp.to_str().expect("path"), false, None);
        assert_eq!(
            std::fs::read_to_string(&hook).expect("preserved hook"),
            "# user-owned hook\n"
        );
        std::fs::remove_dir_all(tmp).expect("remove temp project");
    }

    #[test]
    fn plan_creates_both_when_nothing_exists() {
        let p = plan(".", &|_| false, false);
        assert_eq!(p.len(), 35);
        assert!(p.iter().all(|a| matches!(a, Action::Create { .. })));
        // Schema wiring + agent guide + per-client briefs are the targets.
        let paths: Vec<&str> = p
            .iter()
            .map(|a| match a {
                Action::Create { path, .. } | Action::Skip { path } => path.as_str(),
            })
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("settings.json")));
        assert!(paths.iter().any(|p| p.ends_with("AGENTS.md")));
        assert!(paths.iter().any(|p| p.ends_with("nika.mdc")));
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with(".agents/skills/nika-authoring/SKILL.md"))
        );
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with(".github/copilot-instructions.md"))
        );
        assert!(paths.iter().any(|p| p.ends_with("CLAUDE.md")));
    }

    #[test]
    fn plan_skips_an_existing_file_without_force() {
        // AGENTS.md already there · settings.json/rules absent → one Skip, two Create.
        let p = plan(".", &|path| path.ends_with("AGENTS.md"), false);
        assert!(
            p.iter()
                .any(|a| matches!(a, Action::Skip { path } if path.ends_with("AGENTS.md")))
        );
        assert!(
            p.iter().any(
                |a| matches!(a, Action::Create { path, .. } if path.ends_with("settings.json"))
            )
        );
        assert!(
            p.iter()
                .any(|a| matches!(a, Action::Create { path, .. } if path.ends_with("nika.mdc")))
        );
    }

    #[test]
    fn force_overwrites_everything() {
        let p = plan(".", &|_| true, true);
        assert!(p.iter().all(|a| matches!(a, Action::Create { .. })));
    }

    #[test]
    fn join_respects_the_target_dir() {
        let p = plan("/tmp/proj", &|_| false, false);
        assert!(p.iter().any(|a| matches!(a, Action::Create { path, .. }
                if path == "/tmp/proj/.vscode/settings.json")));
        assert!(p.iter().any(|a| matches!(a, Action::Create { path, .. }
                if path == "/tmp/proj/.cursor/rules/nika.mdc")));
    }

    #[test]
    fn render_marks_created_and_skipped() {
        let out = render(&[
            ('✔', "created .vscode/settings.json".to_owned()),
            (
                '·',
                "skipped AGENTS.md (exists · --force to overwrite)".to_owned(),
            ),
        ]);
        assert!(out.contains("✔ created"));
        assert!(out.contains("· skipped"));
    }

    /// The clap vocabulary and the recipe register can never drift —
    /// same names, same order.
    #[test]
    fn recipe_names_mirror_the_register() {
        let register: Vec<&str> = recipes::RECIPES.iter().map(|r| r.name).collect();
        assert_eq!(RECIPE_NAMES.to_vec(), register);
    }

    /// `--recipe agentic --yes` scaffolds the curriculum, audits every
    /// file, and tailors the hand-off to the first workflow.
    #[test]
    fn scripted_recipe_scaffolds_audits_and_hands_over() {
        let tmp = std::env::temp_dir().join(format!("nika-init-recipe-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let out = run(
            tmp.to_str().expect("utf8"),
            false,
            true,
            Some("agentic"),
            None,
            &[],
            PLAIN,
        );
        assert_eq!(out.code, codes::OK, "{}", out.text);
        assert!(
            out.text.contains("✔ created workflows/01-hello-chain.nika"),
            "{}",
            out.text
        );
        assert!(
            out.text.matches("audited").count() >= 4,
            "all four workflows audited: {}",
            out.text
        );
        assert!(
            out.text
                .contains("nika run workflows/01-hello-chain.nika --model mock/echo"),
            "the hand-off names the first workflow: {}",
            out.text
        );
        assert!(tmp.join("workflows/04-agent-loop.nika").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// `--recipe starter -y` used to land byte-identical to `--recipe
    /// minimal`, output included (measured 2026-08-03) — starter's whole
    /// content is the three-question conversation, which a pipe cannot
    /// answer. A flag that silently becomes another flag is the one thing
    /// a scriptable twin must never do, so the run says which half landed.
    ///
    /// The control is the point: minimal must NOT carry the note, or the
    /// assertion above would pass on a message printed unconditionally.
    #[test]
    fn scripted_starter_says_which_half_a_pipe_cannot_deliver() {
        let base = std::env::temp_dir().join(format!("nika-init-starter-{}", std::process::id()));
        let mut said = Vec::new();
        for recipe in ["starter", "minimal"] {
            let tmp = base.join(recipe);
            std::fs::remove_dir_all(&tmp).ok();
            std::fs::create_dir_all(&tmp).expect("mkdir");
            let out = run(
                tmp.to_str().expect("utf8"),
                false,
                true,
                Some(recipe),
                None,
                &[],
                PLAIN,
            );
            assert_eq!(out.code, codes::OK, "{}", out.text);
            said.push(out.text);
        }
        let (starter, minimal) = (&said[0], &said[1]);
        assert!(
            starter.contains("a script cannot answer it") && starter.contains("nika compile"),
            "starter names the missing half and the twin: {starter}"
        );
        assert!(
            !minimal.contains("a script cannot answer it"),
            "the note is starter's alone, not printed to everyone: {minimal}"
        );
        assert_ne!(
            starter, minimal,
            "two different --recipe values must not render the same bytes"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    /// `--theme` stamps the DAG skin into a CREATED settings file — and
    /// the schema wiring survives the stamp.
    #[test]
    fn scripted_theme_stamps_the_settings() {
        let tmp = std::env::temp_dir().join(format!("nika-init-theme-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let out = run(
            tmp.to_str().expect("utf8"),
            false,
            true,
            None,
            Some(CanvasTheme::Nika),
            &[],
            PLAIN,
        );
        assert_eq!(out.code, codes::OK, "{}", out.text);
        let settings = std::fs::read_to_string(tmp.join(".vscode/settings.json")).expect("written");
        let parsed: serde_json::Value = serde_json::from_str(&settings).expect("valid json");
        assert_eq!(
            parsed.get("nika.dag.theme").and_then(|v| v.as_str()),
            Some("nika")
        );
        assert!(parsed.get("yaml.schemas").is_some());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// An existing settings file is SKIPPED even when `--theme` asks for
    /// a stamp — the human keeps the hand; the skip line says so.
    #[test]
    fn theme_never_clobbers_an_existing_settings_file() {
        let tmp = std::env::temp_dir().join(format!("nika-init-keep-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(tmp.join(".vscode")).expect("mkdir");
        std::fs::write(tmp.join(".vscode/settings.json"), "{\"mine\": true}\n").expect("seed");
        let out = run(
            tmp.to_str().expect("utf8"),
            false,
            true,
            None,
            Some(CanvasTheme::Phosphor),
            &[],
            PLAIN,
        );
        assert_eq!(out.code, codes::OK, "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(tmp.join(".vscode/settings.json")).expect("read"),
            "{\"mine\": true}\n",
            "skipped = untouched"
        );
        assert!(out.text.contains("skipped"), "{}", out.text);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The exec-bit stamp survives refactors — Cursor spawns the
    /// seatbelt scripts directly, so a write path that loses the bit
    /// ships dead hooks to every fresh repo (unix).
    #[cfg(unix)]
    #[test]
    fn scaffolded_hook_scripts_are_executable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("nika-execbit-{}", std::process::id()));
        let dir = tmp.to_string_lossy().into_owned();
        let rows = apply_briefs(&dir, true, None);
        let mode = std::fs::metadata(tmp.join(".cursor/hooks-nika/guard-run.sh"))
            .expect("guard-run.sh written")
            .permissions()
            .mode();
        let plain = std::fs::metadata(tmp.join(".cursor/hooks.json"))
            .expect("hooks.json written")
            .permissions()
            .mode();
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            rows.iter()
                .all(|(_, o)| !matches!(o, BriefOutcome::Failed(_))),
            "all briefs land"
        );
        assert_eq!(mode & 0o111, 0o111, "script carries the exec bit");
        assert_eq!(plain & 0o111, 0, "manifest stays a plain file");
    }
}
