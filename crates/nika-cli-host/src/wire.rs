// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! `nika wire` — consent-based agent/editor MCP wiring.
//!
//! Installers stay conservative and never mutate editor state implicitly. This
//! verb is the explicit, idempotent bridge: it adds or repairs the Nika MCP
//! entry while preserving every unrelated server in the host config.
//!
//! The door (H7 · audit UX 2026-07-30): preview (`--dry-run`) → detected
//! (`wire detected` — only what this machine shows) → one client by name.
//! `all` is the advanced door: it rewrites every known client config, so it
//! owes the operator a preview, a live `y` on a terminal, or an explicit
//! `--yes` in a pipe — never a silent sweep.

use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde_json::{Map, Value};

use crate::output::VerbOutput;

mod settled;
use settled::{Settled, landing};

const NIKA_MCP_ARGS: [&str; 1] = ["mcp"];

/// One variant per client `nika wire` knows how to write — the clap
/// surface IS this enum (`ValueEnum` · kebab-case), so a new client lands
/// in this one file: variant + writer arm + tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum WireTarget {
    Cursor,
    Vscode,
    Windsurf,
    Claude,
    ClaudeDesktop,
    Cline,
    Codex,
    Continue,
    Zed,
    Opencode,
    Hermes,
    Gemini,
    Qwen,
    Lmstudio,
    Junie,
    Grok,
    Antigravity,
    Kimi,
    Kiro,
    Copilot,
    Amp,
    /// Only the clients THIS machine shows (the probe's presence truth)
    /// — the recommended door: `wire detected --dry-run`, then
    /// `wire detected`.
    Detected,
    /// Every supported client — the advanced door: preview with
    /// `--dry-run`, confirm live on a terminal, or pass `--yes` in a pipe.
    All,
}

impl WireTarget {
    /// The client's name as the verb spells it (`claude-desktop` ·
    /// `lmstudio`): read off the clap surface, so a label, a registry id
    /// and a detection row can never spell a client two ways.
    pub(crate) fn id(self) -> String {
        self.to_possible_value()
            .map(|value| value.get_name().to_owned())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WireAction {
    Created(String),
    Updated(String),
    Migrated(String),
    Current(String),
    Skipped(String),
    /// The config exists but cannot be rewritten losslessly (Zed's
    /// settings.json is JSONC — comments would be destroyed): hand the
    /// user the exact snippet instead. A successful outcome, not a skip.
    Manual(String),
}

/// The `wire` door's knobs (H7 · audit UX 2026-07-30): a preview that
/// never writes, and the consent `all` owes the operator.
#[derive(Clone, Copy, Debug, Default)]
pub struct WireOptions {
    /// Plan only — render created/updated/current/manual per client,
    /// write nothing (the `run --dry-run` law: plan only, zero effects).
    pub dry_run: bool,
    /// Consent to `all` without a prompt (scripts · CI — a terminal asks).
    pub yes: bool,
    /// Both ends of the conversation are a terminal: `all` may ask
    /// instead of refusing (the clig.dev interactivity rule).
    pub interactive: bool,
}

/// The compat door (init's injected wire effect): a terminal may consent
/// live, a pipe may not — everything else keeps the scripted defaults.
#[must_use]
pub fn run(target: WireTarget, dir: &str) -> VerbOutput {
    run_with(
        target,
        dir,
        WireOptions {
            interactive: interactive_terminal(),
            ..WireOptions::default()
        },
    )
}

/// The full door (H7): `detected` wires only what the machine shows,
/// `dry_run` renders the plan and writes nothing, and `all` — the
/// every-client sweep — owes a preview, a live `y`, or an explicit
/// `--yes`. A bare `all` in a pipe is refused, naming the right door.
#[must_use]
pub fn run_with(target: WireTarget, dir: &str, options: WireOptions) -> VerbOutput {
    run_in(target, crate::probe::home_dir().as_deref(), dir, options)
}

/// [`run_with`] on a named home: the environment is read once, at the
/// door, and every path below is resolved from what the door was handed.
fn run_in(target: WireTarget, home: Option<&Path>, dir: &str, options: WireOptions) -> VerbOutput {
    if target == WireTarget::All {
        match all_consent(options.dry_run, options.yes, options.interactive) {
            AllConsent::Refuse => {
                return VerbOutput::env(format!(
                    "wire all rewrites {} client configs and nothing asked\n  \
                     preview:     nika wire all --dry-run\n  \
                     recommended: nika wire detected  (only what this machine has)\n  \
                     consent:     nika wire all --yes",
                    expand_target(WireTarget::All).len()
                ));
            }
            AllConsent::Prompt => {
                use std::io::Write as _;
                let targets = expand_target(target);
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{}", render_plan(&collect_plan(&targets, home, dir)));
                if !ask_consent(&mut out, targets.len()) {
                    return VerbOutput::ok(
                        "aborted · nothing written — `nika wire detected` wires only \
                         what this machine has"
                            .to_owned(),
                    );
                }
            }
            AllConsent::Proceed => {}
        }
    }

    let targets = resolve_targets(target, home, dir);
    if targets.is_empty() {
        return VerbOutput::ok(
            "no MCP client detected on this machine — `nika wire all --dry-run` \
             previews every supported client · `nika wire <client>` wires one by name"
                .to_owned(),
        );
    }

    if target == WireTarget::Detected {
        return wire_detected(&targets, home, dir, options.dry_run);
    }
    wire_targets(&targets, home, dir, options.dry_run)
}

/// The `detected` door wires host by host. Nobody named these targets,
/// the machine did: a config its writer refuses (malformed, a server
/// table that is not one) is reported on its own line and left as it
/// is, the other hosts are wired, and the exit is ENV so a script sees
/// that one was skipped. A preview is that plan alone, on the same exit.
fn wire_detected(
    targets: &[WireTarget],
    home: Option<&Path>,
    dir: &str,
    dry_run: bool,
) -> VerbOutput {
    let mut actions = plan_detected(targets, home, dir);
    if !dry_run {
        for (one, action) in targets.iter().zip(&mut actions) {
            // Refused or already wired: left exactly as it is.
            if !matches!(action, WireAction::Skipped(_) | WireAction::Current(_)) {
                *action = wire_one(*one, home, dir, false).unwrap_or_else(WireAction::Skipped);
            }
        }
    }
    let skipped = actions
        .iter()
        .any(|action| matches!(action, WireAction::Skipped(_)));
    let text = if dry_run {
        render_plan(&actions)
    } else {
        render(&actions)
    };
    if skipped {
        VerbOutput::env(text)
    } else {
        VerbOutput::ok(text)
    }
}

/// Wire the `targets` a person named (a client, or `all`), preflighted
/// as one: every client is planned before the first write, and one the
/// plan cannot wire as it is (a malformed config, a server table that is
/// not an object) leaves every config untouched, exit ENV. A preview is
/// that plan alone.
fn wire_targets(
    targets: &[WireTarget],
    home: Option<&Path>,
    dir: &str,
    dry_run: bool,
) -> VerbOutput {
    let plan = collect_plan(targets, home, dir);
    let refused = plan
        .iter()
        .any(|action| matches!(action, WireAction::Skipped(_)));
    if dry_run {
        let text = render_plan(&plan);
        return if refused {
            VerbOutput::env(text)
        } else {
            VerbOutput::ok(text)
        };
    }
    if refused {
        return VerbOutput::env(render_refused(&plan));
    }

    let mut actions = Vec::new();
    let mut failed = false;
    for one in targets {
        match wire_one(*one, home, dir, false) {
            Ok(action) => actions.push(action),
            Err(message) => {
                failed = true;
                actions.push(WireAction::Skipped(message));
            }
        }
    }
    let text = render(&actions);
    if failed {
        VerbOutput::env(text)
    } else {
        VerbOutput::ok(text)
    }
}

/// The `all` consent verdict (pure — the prompt I/O lives in
/// [`run_with`]; the `model pull` confirm-gate precedent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllConsent {
    /// `--dry-run` (nothing to consent to) or an explicit `--yes`.
    Proceed,
    /// A terminal: show the plan, then ask.
    Prompt,
    /// No terminal, no `--yes`: refuse, naming the right door.
    Refuse,
}

fn all_consent(dry_run: bool, yes: bool, interactive: bool) -> AllConsent {
    if dry_run || yes {
        AllConsent::Proceed
    } else if interactive {
        AllConsent::Prompt
    } else {
        AllConsent::Refuse
    }
}

/// Both ends of a conversation present? (the prompt needs stdin AND a
/// terminal to ask on — the `model pull` rule).
fn interactive_terminal() -> bool {
    use std::io::IsTerminal as _;
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Ask on the conversation stream, read one line from stdin — `y`/`yes`
/// applies the plan, anything else (or an unreadable answer) aborts
/// without a write. The prompt rides the SAME `Write` seam the init
/// wizard uses (this crate bans the `eprint!` macros).
fn ask_consent(out: &mut impl std::io::Write, count: usize) -> bool {
    let _ = write!(out, "wire all: apply to {count} client configs? [y/N] ");
    let _ = out.flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let answer = line.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

fn resolve_targets(target: WireTarget, home: Option<&Path>, dir: &str) -> Vec<WireTarget> {
    match target {
        WireTarget::Detected => detected_targets(home, dir),
        other => expand_target(other),
    }
}

/// The clients this machine SHOWS: those whose config — the one file
/// [`config_path`] hands the writer — is already there, whatever it
/// holds. They ride the shared [`crate::detect`] rows for the registry
/// gate (a client dropped from the matrix leaves `doctor` and `wire`
/// together), but not for every path a row knows: `doctor` also reports
/// the project `.cursor/mcp.json` Cursor reads, and a repository that
/// ships one does not put Cursor on this machine. Wiring on that
/// sighting would create a home config for a host that is absent.
fn detected_targets(home: Option<&Path>, dir: &str) -> Vec<WireTarget> {
    home.map(|home| detected_targets_in(home, Path::new(dir)))
        .unwrap_or_default()
}

fn detected_targets_in(home: &Path, dir: &Path) -> Vec<WireTarget> {
    crate::detect::sights(Some(home), dir)
        .iter()
        .map(|sight| sight.target)
        .filter(|target| config_path(*target, Some(home), dir).is_ok_and(|path| path.exists()))
        .collect()
}

/// What `detected` would do to each host it sees, nothing written: each
/// host as [`judged`], so an entry that already works is current as it
/// stands and the run leaves it alone.
fn plan_detected(targets: &[WireTarget], home: Option<&Path>, dir: &str) -> Vec<WireAction> {
    targets
        .iter()
        .map(|one| {
            settled_path(*one, home, Path::new(dir))
                .and_then(|(path, _)| judged(*one, &path))
                .unwrap_or_else(WireAction::Skipped)
        })
        .collect()
}

/// The plan half of a named run: every outcome, zero writes (the
/// interactive `all` preview reads it before asking).
fn collect_plan(targets: &[WireTarget], home: Option<&Path>, dir: &str) -> Vec<WireAction> {
    targets
        .iter()
        .map(|one| match wire_one(*one, home, dir, true) {
            Ok(action) => action,
            Err(message) => WireAction::Skipped(message),
        })
        .collect()
}

pub(crate) fn expand_target(target: WireTarget) -> Vec<WireTarget> {
    match target {
        // Every concrete client, in declaration order (the clap registry
        // is the one list — no hand-maintained twin to drift). `All` and
        // `Detected` are DOORS, not clients.
        WireTarget::All => WireTarget::value_variants()
            .iter()
            .copied()
            .filter(|t| !matches!(t, WireTarget::All | WireTarget::Detected))
            .collect(),
        other => vec![other],
    }
}

/// Where each client keeps the config `nika wire` writes — THE one
/// answer. The writers patch this path and [`crate::detect`] looks for
/// this path, so a client cannot be wired without being seen, and a new
/// [`WireTarget`] does not compile until it says where it lives. `home`
/// is the user's home (without one, only the project-anchored clients
/// resolve) and `dir` the project.
pub(crate) fn config_path(
    target: WireTarget,
    home: Option<&Path>,
    dir: &Path,
) -> Result<PathBuf, String> {
    config_place(target, home, dir).map(|(_, _, path)| path)
}

/// Who owns a config's folder: the project, for a file the writer places
/// in the repository, or the user's home, for every other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Project,
    Home,
}

/// [`config_path`] with the folder that OWNS the config: the project for
/// a file the writer places in it, the home for every other. The same
/// arm says both, so a target cannot name a path without naming the
/// folder a link in that path must stay inside ([`settled_path`]).
fn config_place<'a>(
    target: WireTarget,
    home: Option<&'a Path>,
    dir: &'a Path,
) -> Result<(Owner, &'a Path, PathBuf), String> {
    let file =
        |base: &'a Path, parts: &[&str]| parts.iter().fold(base.to_path_buf(), |p, s| p.join(s));
    let user = || home.ok_or_else(no_home);
    let join = |base: &'a Path, parts: &[&str]| (Owner::Home, base, file(base, parts));
    let in_project = |parts: &[&str]| (Owner::Project, dir, file(dir, parts));
    Ok(match target {
        // The user-home `mcpServers` family — one shape, per-client paths
        // (gemini: shared settings.json, server names must not contain `_`
        // — upstream policy parser splits on it, `nika` is safe · qwen: the
        // gemini-cli fork, same key, its own dotdir · antigravity: standalone
        // `mcp_config.json` under `~/.gemini/config/` · kimi: two-level
        // `mcp.json`, machine wiring writes the user file · kiro: the Amazon Q
        // rebrand, `.kiro` wins over the legacy `~/.aws/amazonq/`).
        WireTarget::Cursor => join(user()?, &[".cursor", "mcp.json"]),
        WireTarget::Claude => join(user()?, &[".claude.json"]),
        WireTarget::Windsurf => join(user()?, &[".codeium", "windsurf", "mcp_config.json"]),
        WireTarget::Gemini => join(user()?, &[".gemini", "settings.json"]),
        WireTarget::Qwen => join(user()?, &[".qwen", "settings.json"]),
        WireTarget::Antigravity => join(user()?, &[".gemini", "config", "mcp_config.json"]),
        WireTarget::Kimi => join(user()?, &[".kimi-code", "mcp.json"]),
        WireTarget::Kiro => join(user()?, &[".kiro", "settings", "mcp.json"]),
        WireTarget::Vscode => in_project(&[".vscode", "mcp.json"]),
        // Claude DESKTOP is a different app with a different config file
        // than Claude Code's ~/.claude.json — the wave-3 double gap (#449).
        WireTarget::ClaudeDesktop => (Owner::Home, user()?, claude_desktop_config_path(home)?),
        // Cline: the resolver below picks the live globalStorage file when
        // a host IDE has it, else the stable `~/.cline/data/settings/` home
        // (#449 wave-3b).
        WireTarget::Cline => (Owner::Home, user()?, cline_settings_path(home)?),
        // Codex CLI, and Grok Build on the same TOML shape.
        WireTarget::Codex => join(user()?, &[".codex", "config.toml"]),
        WireTarget::Grok => join(user()?, &[".grok", "config.toml"]),
        // Continue scans `~/.continue/mcpServers/*.json` (the Claude-Desktop
        // JSON shape, name-keyed) — an OWN-FILE write: the user's
        // comment-bearing config.yaml is never touched (the Zed lesson,
        // applied preemptively). So this file says Continue was WIRED, not
        // that it is installed: detection sees Continue once wired, and
        // `nika wire continue` is the door before that.
        WireTarget::Continue => join(user()?, &[".continue", "mcpServers", "nika.json"]),
        // Zed keeps its settings under ~/.config on EVERY platform (macOS
        // included — deliberate upstream choice, zed.dev/docs).
        WireTarget::Zed => join(user()?, &[".config", "zed", "settings.json"]),
        // OpenCode merges the PROJECT-local opencode.json over the global
        // one (opencode.ai/docs/mcp-servers) — the project file is the
        // least-privilege home (the repo the oracle serves).
        WireTarget::Opencode => in_project(&["opencode.json"]),
        WireTarget::Hermes => join(user()?, &[".hermes", "config.yaml"]),
        WireTarget::Lmstudio => (Owner::Home, user()?, lmstudio_mcp_path(home)?),
        // Junie reads project-scope `.junie/mcp/mcp.json` (`mcpServers`
        // root · junie-cli-mcp-configuration.html); the global
        // `~/.junie/mcp/mcp.json` exists but the project file is the
        // least-privilege home, same reasoning as OpenCode above.
        WireTarget::Junie => in_project(&[".junie", "mcp", "mcp.json"]),
        WireTarget::Copilot => join(user()?, &[".copilot", "mcp-config.json"]),
        WireTarget::Amp => join(user()?, &[".config", "amp", "settings.json"]),
        WireTarget::All | WireTarget::Detected => return Err(DOOR.to_owned()),
    })
}

/// `all` and `detected` name no config of their own: they are expanded
/// into clients before any path or writer is asked for.
const DOOR: &str = "wire doors must be resolved before dispatch";

fn no_home() -> String {
    "cannot find HOME/USERPROFILE for editor wiring".to_owned()
}

/// `target`'s config path, once it is known to stay inside the folder
/// that owns it. A settings path can be a link, or sit under a linked
/// folder, and a link is followed only while what it leads to is still
/// in the project (a project file) or in the home (every other). A
/// repository can ship `opencode.json` as a link to any file of the
/// account's: followed, `wire` would write there. Refused instead, by
/// the plan and by the run, before the writer reads or writes anything.
/// A project config is held tighter still ([`project_way`]): a link on the
/// way, at a folder or at the config's own name, is read one step at a time
/// and followed only while every place it leads through is inside the
/// project (or above it, on the way down), to something that is there and
/// outside its `.git` folder, and the config's own link only to a regular
/// file. What comes back is the path and where a write to it lands, real
/// ([`settled::landing`]), itself judged before it is held ([`held_inside`]);
/// the writer looks again, refuses a write that would land anywhere else by
/// then ([`settled::Settled`]), and writes to the real place it compared
/// ([`destination`]), so no link is followed again. The looks are by path:
/// a process that swaps a real folder on that place for a link between the
/// last look and the rename could still redirect the write (`nika init`
/// documents the same gap; closing it needs every step held by descriptor).
fn settled_path(
    target: WireTarget,
    home: Option<&Path>,
    dir: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    settled_with(target, home, dir, &landing)
}

/// [`settled_path`] with the look that finds where a write lands, so a test
/// can stand in for a place that changed after the two checks before it.
fn settled_with(
    target: WireTarget,
    home: Option<&Path>,
    dir: &Path,
    look: &dyn Fn(&Path) -> std::io::Result<PathBuf>,
) -> Result<(PathBuf, PathBuf), String> {
    let (owner, base, path) = config_place(target, home, dir)?;
    // A project config's way is walked first, one step at a time: a link
    // there that loops, leads nowhere or leads out is named as such, with
    // its way out, before a look that resolves the whole path at once
    // could only give the system's own reason.
    match owner {
        Owner::Project => project_way(base, &path).map_err(|why| why.to_string()),
        Owner::Home => Ok(()),
    }
    .and_then(|()| stays_inside(base, &path))
    .and_then(|()| look(&path).map_err(|e| format!("{}: {e}", path.display())))
    .and_then(|lands| held_inside(base, &path, lands))
    .map_err(|why| format!("{}: {why}", target.id()))
    .map(|lands| (path, lands))
}

/// The place a write to `path` lands, judged by itself before the writer
/// holds it: it comes from a look of its own, after the ones that judged
/// the path, so it too must be inside the folder that owns the config
/// (`owner`, real), below it by plain names only (no `..`, no `.`), and
/// outside a `.git` folder there. Names are compared as components, `.git`
/// without regard to ASCII case.
fn held_inside(owner: &Path, path: &Path, lands: PathBuf) -> Result<PathBuf, String> {
    let inside = nearest_real(owner).map_err(|e| format!("{}: {e}", path.display()))?;
    let Ok(within) = lands.strip_prefix(&inside) else {
        return Err(format!(
            "{}: a write would land at {}, outside {}; nika wire leaves it as it is · \
             run nika wire again",
            path.display(),
            lands.display(),
            inside.display()
        ));
    };
    if within.as_os_str().is_empty()
        || !within
            .components()
            .all(|name| matches!(name, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "{}: a write would land at {}, not a file named plainly inside {}; nika wire \
             leaves it as it is · run nika wire again",
            path.display(),
            lands.display(),
            inside.display()
        ));
    }
    if within
        .components()
        .any(|name| name.as_os_str().eq_ignore_ascii_case(".git"))
    {
        return Err(format!(
            "{}: a write would land at {}, in a .git folder; nika wire never writes there · \
             run nika wire again",
            path.display(),
            lands.display()
        ));
    }
    Ok(lands)
}

/// Why `wire` leaves a project config as it is: a link on the way to it
/// that it does not follow. A repository decides every name in it, so a
/// link there is followed only to a place the project owns.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProjectLink {
    /// A link on the way (a folder, or the config's own name) leads to
    /// nothing: following it would create a file where it points.
    Dangling(PathBuf),
    /// A link on the way leads outside the project.
    Outside(PathBuf),
    /// A link on the way never ends: it leads back to itself, or through
    /// more links than a walk follows ([`nika_fs::MAX_LINKS`]).
    Loop(PathBuf),
    /// A link on the way leads into the project's `.git` folder.
    IntoGit(PathBuf),
    /// The config's own name links to something that is not a regular
    /// file (a folder, a FIFO, a device).
    NotAFile(PathBuf),
    /// An entry on the way could not be read.
    Unreadable(PathBuf, String),
}

impl std::fmt::Display for ProjectLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dangling(at) => write!(
                f,
                "{} is a link that leads nowhere; nika wire never creates what a link points \
                 at: point it at a file inside the project, or remove it",
                at.display()
            ),
            Self::Outside(at) => write!(
                f,
                "{} is a link that leads outside the project; nika wire writes a project \
                 config only inside the project: point it inside the project, or remove it",
                at.display()
            ),
            Self::Loop(at) => write!(
                f,
                "{} is a link loop: following it never ends; nika wire follows a link only \
                 to a place it reaches: point it at a file inside the project, or remove it",
                at.display()
            ),
            Self::IntoGit(at) => write!(
                f,
                "{} is a link into the project's .git folder; nika wire never writes there: \
                 point it at a file outside .git, or remove it",
                at.display()
            ),
            Self::NotAFile(at) => write!(
                f,
                "{} is a link to something that is not a regular file; nika wire writes a \
                 project config only into a file: point it at one, or remove it",
                at.display()
            ),
            Self::Unreadable(at, why) => write!(f, "{}: cannot read: {why}", at.display()),
        }
    }
}

/// The way from `project` to `path`, one name at a time. A link, at a
/// folder or at the config's own name, is followed only to what is there,
/// inside the project and outside its `.git` folder ([`followed`]), and
/// the config's own link only to a regular file, which the writer then
/// replaces where it really is ([`destination`]): the link stays. The
/// first name that is not there ends the walk: nothing below it exists to
/// be a link, and the writer creates it.
fn project_way(project: &Path, path: &Path) -> Result<(), ProjectLink> {
    let Ok(rest) = path.strip_prefix(project) else {
        return Ok(()); // config_place joins every project config under it
    };
    let unreadable =
        |at: &Path, e: std::io::Error| ProjectLink::Unreadable(at.into(), e.to_string());
    let inside = nearest_real(project).map_err(|e| unreadable(project, e))?;
    let names: Vec<_> = rest.components().collect();
    let mut at = project.to_path_buf();
    for (index, name) in names.iter().enumerate() {
        at.push(name);
        let meta = match std::fs::symlink_metadata(&at) {
            Ok(meta) => meta,
            // Nothing there, or a file where a folder should be: the
            // writer creates the rest, or says why it cannot.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(());
            }
            Err(e) => return Err(unreadable(&at, e)),
        };
        if !meta.file_type().is_symlink() {
            continue;
        }
        let real = followed(project, &inside, &at)?;
        if index + 1 == names.len() && !std::fs::metadata(&real).is_ok_and(|m| m.is_file()) {
            return Err(ProjectLink::NotAFile(at));
        }
    }
    Ok(())
}

/// Where the link at `at` (below `project`) really leads, found one step at
/// a time ([`nika_fs::resolve_beneath`]): every link on the way is read and
/// followed only while each place it leads through is inside the project
/// (`inside`, real) or one of the folders above it, so a way out and back
/// in is refused where it leaves, since a link outside is its owner's to
/// turn elsewhere between the look and the write. What it reaches must be
/// there and outside the project's `.git` folder. Names are compared as
/// components, `.git` without regard to ASCII case (a case-insensitive
/// volume reaches the same folder by either spelling).
fn followed(project: &Path, inside: &Path, at: &Path) -> Result<PathBuf, ProjectLink> {
    let rest = at.strip_prefix(project).unwrap_or(at);
    let real = match nika_fs::resolve_beneath(project, rest) {
        Ok(nika_fs::Beneath::There(real)) => real,
        Err(nika_fs::Escape::Io(_, e)) => {
            return Err(ProjectLink::Unreadable(at.into(), e.to_string()));
        }
        Err(nika_fs::Escape::Dangling(_)) | Ok(nika_fs::Beneath::Missing { .. }) => {
            return Err(ProjectLink::Dangling(at.into()));
        }
        Err(nika_fs::Escape::Loop(_)) => return Err(ProjectLink::Loop(at.into())),
        // A way out (and back).
        Ok(_) | Err(_) => return Err(ProjectLink::Outside(at.into())),
    };
    let Ok(within) = real.strip_prefix(inside) else {
        return Err(ProjectLink::Outside(at.into()));
    };
    if within
        .components()
        .any(|name| name.as_os_str().eq_ignore_ascii_case(".git"))
    {
        return Err(ProjectLink::IntoGit(at.into()));
    }
    Ok(real)
}

/// Does a write to `path` land inside `owner`? The file's real location
/// when it is there; when it is not (or is a link whose target is gone,
/// which the write replaces), the real location of the folder a creation
/// would land in, so a linked folder cannot redirect a creation either.
/// Real paths are compared by components, never as strings.
fn stays_inside(owner: &Path, path: &Path) -> Result<(), String> {
    let unreadable = |e: std::io::Error| format!("{}: {e}", path.display());
    let inside = nearest_real(owner).map_err(unreadable)?;
    let lands = match std::fs::canonicalize(path) {
        Ok(real) => real,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            nearest_real(path.parent().unwrap_or(path)).map_err(unreadable)?
        }
        Err(e) => return Err(unreadable(e)),
    };
    if lands.starts_with(&inside) {
        return Ok(());
    }
    Err(format!(
        "{} leads to {}, outside {}; nika wire leaves it as it is: \
         point the link inside that folder, or remove it",
        path.display(),
        lands.display(),
        inside.display()
    ))
}

/// The real location of `path`, or of the nearest folder above it that
/// exists: nothing below that one is there to be a link.
fn nearest_real(path: &Path) -> std::io::Result<PathBuf> {
    for above in path.ancestors() {
        match std::fs::canonicalize(above) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            found => return found,
        }
    }
    // A relative path with nothing of it on disk yet: it starts here.
    std::fs::canonicalize(".")
}

fn wire_one(
    target: WireTarget,
    home: Option<&Path>,
    dir: &str,
    dry_run: bool,
) -> Result<WireAction, String> {
    let (path, lands) = settled_path(target, home, Path::new(dir))?;
    let _held = Settled::hold(&path, lands);
    wire_at(target, &path, dry_run)
}

/// The writer half of a wiring: patch the config at `path` the way
/// `target`'s host reads it. The path is [`config_path`]'s, never decided
/// here. A refusal names its host: a read or a write that fails knows
/// only the file, and `detected` prints one line per host.
fn wire_at(target: WireTarget, path: &Path, dry_run: bool) -> Result<WireAction, String> {
    let host = format!("{}: ", target.id());
    patch_at(target, path, dry_run).map_err(|why| {
        if why.starts_with(&host) {
            why
        } else {
            format!("{host}{why}")
        }
    })
}

fn patch_at(target: WireTarget, path: &Path, dry_run: bool) -> Result<WireAction, String> {
    match target {
        WireTarget::Vscode => patch_vscode(path, dry_run),
        // The `[mcp_servers.nika]` table: Codex CLI, and Grok Build, whose
        // native table survives a `[compat.claude] mcps = false` toggle.
        WireTarget::Codex | WireTarget::Grok => patch_toml_mcp(path, &target.id(), dry_run),
        // External writes are NOT hot-reloaded — the hint rides the
        // verdict line (#449 wave-3b).
        WireTarget::Continue => patch_cursor_like(path, "mcpServers", "continue", false, dry_run)
            .map(|a| with_hint(a, "reload Continue (or re-save config.yaml) to pick it up")),
        WireTarget::Zed => patch_zed(path, dry_run),
        WireTarget::Opencode => patch_opencode(path, dry_run),
        WireTarget::Hermes => patch_hermes(path, dry_run),
        // Copilot CLI: the desired matches THEIR writer byte-shape (tools
        // + type local) so a copilot-added entry reads Current, no churn.
        WireTarget::Copilot => patch_copilot(path, dry_run),
        // Amp: the LITERAL dotted "amp.mcpServers" key at the settings
        // root; a JSONC file gets the snippet (the Zed lossless contract).
        WireTarget::Amp => patch_amp(path, dry_run),
        // The Cursor-style `mcpServers` record, whichever file holds it
        // (Cline hot-reloads its own through a chokidar watcher).
        WireTarget::Cursor
        | WireTarget::Claude
        | WireTarget::Windsurf
        | WireTarget::Gemini
        | WireTarget::Qwen
        | WireTarget::Antigravity
        | WireTarget::Kimi
        | WireTarget::Kiro
        | WireTarget::ClaudeDesktop
        | WireTarget::Cline
        | WireTarget::Lmstudio
        | WireTarget::Junie => patch_cursor_like(path, "mcpServers", &target.id(), false, dry_run),
        WireTarget::All | WireTarget::Detected => Err(DOOR.to_owned()),
    }
}

/// `target`'s plan at `path` as the `detected` door judges it, on every
/// host: the writer's own plan, nothing written, except that an entry
/// the writer would only respell is current. A `nika` entry that RUNS
/// ours — a `command` that is a non-empty string, and exactly the args
/// `wire` writes ([`entry_argv`]) — is wired whatever that command's path
/// and whatever keys of its own it holds: one voice with [`reads`] about
/// one entry. A desktop application does not inherit the shell's PATH, so
/// its entry pins an absolute binary on purpose: replacing it with the
/// bare name can break a setup that works. An entry with our args and no
/// command launches nothing, and is repaired like any other. (A client
/// wired BY NAME keeps the strict writer: the person asked for nika's own
/// spelling.)
fn judged(target: WireTarget, path: &Path) -> Result<WireAction, String> {
    match wire_at(target, path, true) {
        Ok(WireAction::Updated(_))
            if entry_argv(target, path)
                .is_some_and(|(command, args)| !command.is_empty() && args == NIKA_MCP_ARGS) =>
        {
            let label = format!("{}: {}", target.id(), path.display());
            Ok(WireAction::Current(label))
        }
        planned => planned,
    }
}

/// How the config at `path` reads for `target`, as `(current, stale)`:
/// [`judged`] current, or stale when the writer would migrate the old
/// `mcp serve --stdio` argv. Nothing is written.
pub(crate) fn reads(target: WireTarget, path: &Path) -> (bool, bool) {
    match judged(target, path) {
        Ok(WireAction::Current(_)) => (true, false),
        Ok(WireAction::Migrated(_)) => (false, true),
        _ => (false, false),
    }
}

/// The `nika` entry `target`'s host reads at `path`, as the command it
/// launches and the args it passes: each writer's table and shape, read
/// back (a test wires every such host and holds this reader to its
/// writer). `None` when the file does not parse, holds no entry, or the
/// entry is not a command line — no `command` string, args that are not
/// an array, or an array holding anything but strings, in JSON and in
/// TOML alike. And for Hermes, whose YAML its writer never rewrites and
/// reads with a predicate of its own.
fn entry_argv(target: WireTarget, path: &Path) -> Option<(String, Vec<String>)> {
    let body = crate::detect::read_config(path).ok()?;
    let table = match target {
        WireTarget::Codex | WireTarget::Grok => {
            let doc: toml_edit::DocumentMut = body.parse().ok()?;
            let entry = doc.get("mcp_servers")?.get("nika")?;
            let command = entry.get("command")?.as_str()?.to_owned();
            return Some((command, toml_args(entry)?));
        }
        WireTarget::Opencode => "mcp",
        WireTarget::Amp => "amp.mcpServers",
        WireTarget::Vscode => "servers",
        WireTarget::Zed => "context_servers",
        WireTarget::Cursor
        | WireTarget::Windsurf
        | WireTarget::Claude
        | WireTarget::ClaudeDesktop
        | WireTarget::Cline
        | WireTarget::Continue
        | WireTarget::Gemini
        | WireTarget::Qwen
        | WireTarget::Lmstudio
        | WireTarget::Junie
        | WireTarget::Antigravity
        | WireTarget::Kimi
        | WireTarget::Kiro
        | WireTarget::Copilot => "mcpServers",
        WireTarget::Hermes | WireTarget::All | WireTarget::Detected => return None,
    };
    let root: Value = serde_json::from_str(&body).ok()?;
    let entry = root.get(table)?.get("nika")?;
    let strings = |value: &Value| -> Option<Vec<String>> {
        value
            .as_array()?
            .iter()
            .map(|arg| arg.as_str().map(str::to_owned))
            .collect()
    };
    if target == WireTarget::Opencode {
        // OpenCode keeps the whole argv in `command`: its head is the
        // command, its tail the args.
        let mut argv = strings(entry.get("command")?)?.into_iter();
        let command = argv.next()?;
        return Some((command, argv.collect()));
    }
    let command = entry.get("command")?.as_str()?.to_owned();
    Some((command, strings(entry.get("args")?)?))
}

/// LM Studio documents `~/.lmstudio/mcp.json` (blog v0.3.17 · macOS+Linux ·
/// `%USERPROFILE%\.lmstudio` on Windows), but on some macOS installs the app
/// actually keeps it under `~/.cache/lm-studio/` (lmstudio-bug-tracker#1371,
/// open) — writing the documented path there would wire nothing. Resolution:
/// whichever app-created directory EXISTS wins, documented location first;
/// neither existing falls back to the documented default (created on write).
fn lmstudio_mcp_path(home: Option<&Path>) -> Result<PathBuf, String> {
    home.map(lmstudio_mcp_path_from).ok_or_else(no_home)
}

/// Claude Desktop keeps `claude_desktop_config.json` under the app-config
/// dir (modelcontextprotocol.io quickstart/user · the one client with an
/// official Anthropic connectors directory): macOS
/// `~/Library/Application Support/Claude/` · Windows `%APPDATA%\Claude\` ·
/// Linux `~/.config/Claude/` (community builds — same dir the app uses).
#[allow(clippy::disallowed_methods)]
fn claude_desktop_config_path(home: Option<&Path>) -> Result<PathBuf, String> {
    if cfg!(target_os = "windows")
        && let Some(appdata) = std::env::var_os("APPDATA")
    {
        return Ok(PathBuf::from(appdata)
            .join("Claude")
            .join("claude_desktop_config.json"));
    }
    home.map(claude_desktop_config_path_from)
        .ok_or_else(no_home)
}

fn claude_desktop_config_path_from(home: &Path) -> PathBuf {
    let dir = if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("Claude")
    } else {
        home.join(".config").join("Claude")
    };
    dir.join("claude_desktop_config.json")
}

/// Cline's settings live in TWO generations of home (cline/cline v4.0.8 vs
/// main): today's extension keeps `settings/cline_mcp_settings.json` under
/// the HOST IDE's globalStorage (`saoudrizwan.claude-dev` — the dir name is
/// stable across VS Code · Cursor · Windsurf · `VSCodium` · Insiders); the
/// CLI already reads — and the next extension release migrates to — the
/// stable `~/.cline/data/settings/cline_mcp_settings.json`. Resolution:
/// a host IDE's EXISTING extension dir wins (the live, hot-reloaded file);
/// none existing falls back to the stable home (created on write — correct
/// for the CLI today and the extension after its migration).
fn cline_settings_path(home: Option<&Path>) -> Result<PathBuf, String> {
    home.map(cline_settings_path_from).ok_or_else(no_home)
}

fn cline_settings_path_from(home: &Path) -> PathBuf {
    const HOSTS: [&str; 5] = ["Code", "Cursor", "Windsurf", "VSCodium", "Code - Insiders"];
    for host in HOSTS {
        let root = if cfg!(target_os = "macos") {
            home.join("Library").join("Application Support").join(host)
        } else {
            home.join(".config").join(host)
        };
        let ext = root
            .join("User")
            .join("globalStorage")
            .join("saoudrizwan.claude-dev");
        if ext.is_dir() {
            return ext.join("settings").join("cline_mcp_settings.json");
        }
    }
    home.join(".cline")
        .join("data")
        .join("settings")
        .join("cline_mcp_settings.json")
}

/// Append a post-write hint to a SUCCESSFUL write verdict (`Created` /
/// `Updated`) — for clients that do not hot-reload external writes.
/// `Current` stays bare (already wired, nothing to reload).
fn with_hint(action: WireAction, hint: &str) -> WireAction {
    match action {
        WireAction::Created(label) => WireAction::Created(format!("{label} — {hint}")),
        WireAction::Updated(label) => WireAction::Updated(format!("{label} — {hint}")),
        other => other,
    }
}

fn lmstudio_mcp_path_from(home: &Path) -> PathBuf {
    let documented = home.join(".lmstudio");
    let cache = home.join(".cache").join("lm-studio");
    if !documented.is_dir() && cache.is_dir() {
        return cache.join("mcp.json");
    }
    documented.join("mcp.json")
}

/// Zed reads MCP servers from `context_servers` in `settings.json`
/// (zed.dev/docs/ai/mcp · the `{"command","args","env"}` shape). The file
/// is JSONC — Zed's DEFAULT settings ship with comments — so the contract
/// is: only rewrite a file we can round-trip losslessly (a plain-JSON parse
/// succeeding ⇒ no comments existed); otherwise return
/// [`WireAction::Manual`] with the exact snippet and leave the user's file
/// byte-identical. A file that cannot be read at all is not that case: it
/// is refused with its reason ([`read_plain_json`]).
fn patch_zed(path: &Path, dry_run: bool) -> Result<WireAction, String> {
    let existed = path.exists();
    let label_path = format!("zed: {}", path.display());
    let mut root = if existed {
        let Some(value) = read_plain_json(path)? else {
            return Ok(WireAction::Manual(format!(
                "{label_path} {NOT_PLAIN_JSON} — add this to \
                 \"context_servers\" yourself:\n  \"nika\": {{ \
                 \"command\": \"nika\", \"args\": [\"mcp\"], \"env\": {{}} }}"
            )));
        };
        value
    } else {
        Value::Object(Map::new())
    };

    let object = root
        .as_object_mut()
        .ok_or_else(|| format!("zed: {} is not a JSON object", path.display()))?;
    let servers = server_table(object, "context_servers", "zed", path)?;

    let existing = servers.get("nika").cloned();
    let desired = zed_server();
    let migrated = existing
        .as_ref()
        .is_some_and(crate::detect::is_stale_mcp_server);
    if existing.as_ref() == Some(&desired) {
        return Ok(WireAction::Current(label_path));
    }

    servers.insert("nika".to_owned(), desired);
    if !dry_run {
        write_json(path, &root)?;
    }

    if migrated {
        Ok(WireAction::Migrated(label_path))
    } else if existed {
        Ok(WireAction::Updated(label_path))
    } else {
        Ok(WireAction::Created(label_path))
    }
}

/// The documented Zed entry: `command` + `args` + `env` (the `env` key is
/// in the upstream example for command-based servers — kept for fidelity).
fn zed_server() -> Value {
    let mut server = Map::new();
    server.insert("command".to_owned(), Value::String("nika".to_owned()));
    server.insert(
        "args".to_owned(),
        Value::Array(
            NIKA_MCP_ARGS
                .iter()
                .map(|arg| Value::String((*arg).to_owned()))
                .collect(),
        ),
    );
    server.insert("env".to_owned(), Value::Object(Map::new()));
    Value::Object(server)
}

fn patch_vscode(path: &Path, dry_run: bool) -> Result<WireAction, String> {
    patch_config(path, "servers", "vscode", true, dry_run)
}

fn patch_cursor_like(
    path: &Path,
    server_key: &str,
    label: &str,
    include_type: bool,
    dry_run: bool,
) -> Result<WireAction, String> {
    patch_config(path, server_key, label, include_type, dry_run)
}

fn patch_config(
    path: &Path,
    server_key: &str,
    label: &str,
    include_type: bool,
    dry_run: bool,
) -> Result<WireAction, String> {
    let existed = path.exists();
    let mut root = read_existing(path, existed)?;

    let object = root
        .as_object_mut()
        .ok_or_else(|| format!("{label}: {} is not a JSON object", path.display()))?;
    let servers = server_table(object, server_key, label, path)?;

    let existing = servers.get("nika").cloned();
    let desired = nika_server(include_type);
    let migrated = existing
        .as_ref()
        .is_some_and(crate::detect::is_stale_mcp_server);
    let current = existing.as_ref().is_some_and(|value| *value == desired);

    if current {
        return Ok(WireAction::Current(format!("{label}: {}", path.display())));
    }

    servers.insert("nika".to_owned(), desired);
    if !dry_run {
        write_json(path, &root)?;
    }

    let label_path = format!("{label}: {}", path.display());
    if migrated {
        Ok(WireAction::Migrated(label_path))
    } else if existed {
        Ok(WireAction::Updated(label_path))
    } else {
        Ok(WireAction::Created(label_path))
    }
}

/// The `[mcp_servers.nika]` TOML family — Codex CLI (`~/.codex/config.toml`)
/// and Grok Build (`~/.grok/config.toml`) share the exact table shape.
/// `toml_edit` keeps the user's comments and unrelated tables intact.
fn patch_toml_mcp(path: &Path, label: &str, dry_run: bool) -> Result<WireAction, String> {
    let existed = path.exists();
    let body = if existed {
        crate::detect::read_config(path)?
    } else {
        String::new()
    };
    let mut doc: toml_edit::DocumentMut = body
        .parse()
        .map_err(|e| format!("{}: malformed TOML: {e}", path.display()))?;

    // A value that is not a table is the user's: refused, never replaced.
    let servers = doc
        .entry("mcp_servers")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .ok_or_else(|| {
            format!(
                "{label}: `mcp_servers` in {} is not a TOML table; nika wire leaves it as it is",
                path.display()
            )
        })?;
    servers.set_implicit(true);

    let existing = servers.get("nika");
    let current = existing.is_some_and(toml_entry_is_current);
    let migrated = existing.is_some_and(toml_entry_is_stale);

    let label_path = format!("{label}: {}", path.display());
    if current {
        return Ok(WireAction::Current(label_path));
    }

    let mut entry = toml_edit::Table::new();
    entry.insert("command", toml_edit::value("nika"));
    let mut args = toml_edit::Array::new();
    for arg in NIKA_MCP_ARGS {
        args.push(arg);
    }
    entry.insert("args", toml_edit::value(args));
    servers.insert("nika", toml_edit::Item::Table(entry));

    if !dry_run {
        write_atomic(path, &doc.to_string())?;
    }

    if migrated {
        Ok(WireAction::Migrated(label_path))
    } else if existed {
        Ok(WireAction::Updated(label_path))
    } else {
        Ok(WireAction::Created(label_path))
    }
}

/// The args of a `[mcp_servers.nika]` entry: `None` unless `args` is an
/// array of strings and nothing else. An element that is not a string is
/// never dropped to make the rest look like ours.
fn toml_args(item: &toml_edit::Item) -> Option<Vec<String>> {
    item.get("args")?
        .as_array()?
        .iter()
        .map(|arg| arg.as_str().map(str::to_owned))
        .collect()
}

fn toml_entry_is_current(item: &toml_edit::Item) -> bool {
    item.get("command").and_then(toml_edit::Item::as_str) == Some("nika")
        && toml_args(item).is_some_and(|args| args == NIKA_MCP_ARGS)
}

fn toml_entry_is_stale(item: &toml_edit::Item) -> bool {
    toml_args(item).is_some_and(|args| args == ["mcp", "serve", "--stdio"])
}

/// `OpenCode` wires MCP through `opencode.json` — its OWN shape (`mcp.nika`
/// with `type: local` + the WHOLE argv in `command`), not the
/// `mcpServers` family — so it gets a dedicated desired-value while
/// reusing the same read/merge/write mechanics (#330).
fn patch_opencode(path: &Path, dry_run: bool) -> Result<WireAction, String> {
    let existed = path.exists();
    let mut root = read_existing(path, existed)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| format!("opencode: {} is not a JSON object", path.display()))?;
    let servers = server_table(object, "mcp", "opencode", path)?;

    let desired = serde_json::json!({
        "type": "local",
        "command": ["nika", "mcp"],
        "enabled": true,
    });
    let existing = servers.get("nika").cloned();
    if existing.as_ref() == Some(&desired) {
        return Ok(WireAction::Current(format!("opencode: {}", path.display())));
    }
    servers.insert("nika".to_owned(), desired);
    if !dry_run {
        write_json(path, &root)?;
    }
    let label_path = format!("opencode: {}", path.display());
    if existed {
        Ok(WireAction::Updated(label_path))
    } else {
        Ok(WireAction::Created(label_path))
    }
}

/// The exact Hermes MCP block (`~/.hermes/config.yaml` ·
/// `hermes_cli/mcp_config.py` accepts any PATH binary · `/reload-mcp`
/// refreshes).
const HERMES_SNIPPET: &str =
    "mcp_servers:\n  nika:\n    command: nika\n    args: [mcp]\n    timeout: 120\n";

/// Hermes reads `~/.hermes/config.yaml` — YAML, where user files carry
/// comments and anchors no serializer round-trips. Same contract as Zed
/// (the JSONC precedent): CREATE the file when missing, recognize a
/// CURRENT entry, otherwise hand back the exact snippet
/// ([`WireAction::Manual`]) and leave the user's file byte-identical.
fn patch_hermes(path: &Path, dry_run: bool) -> Result<WireAction, String> {
    if !path.exists() {
        if !dry_run {
            write_atomic(path, HERMES_SNIPPET)?;
        }
        return Ok(WireAction::Created(format!("hermes: {}", path.display())));
    }
    let body = crate::detect::read_config(path)?;
    // CURRENT means the canonical block is present verbatim-modulo-indent
    // — the shared `detect::hermes_recognized` predicate (a `nika:` server
    // whose command line names the binary). Anything else is the
    // operator’s file to edit — print the snippet, touch nothing.
    let has_nika = crate::detect::hermes_recognized(&body);
    if has_nika {
        return Ok(WireAction::Current(format!("hermes: {}", path.display())));
    }
    Ok(WireAction::Manual(format!(
        "hermes: {} exists — add under `mcp_servers:` (then /reload-mcp):\n{}",
        path.display(),
        HERMES_SNIPPET
    )))
}

/// Copilot CLI's own `mcp add` shape — `tools: ["*"]` + `type: "local"`
/// alongside command/args. Matching their writer keeps a copilot-added
/// entry `Current` instead of churning it on every `nika wire copilot`.
fn copilot_server() -> Value {
    let mut server = Map::new();
    server.insert(
        "tools".to_owned(),
        Value::Array(vec![Value::String("*".to_owned())]),
    );
    server.insert("type".to_owned(), Value::String("local".to_owned()));
    server.insert("command".to_owned(), Value::String("nika".to_owned()));
    server.insert(
        "args".to_owned(),
        Value::Array(
            NIKA_MCP_ARGS
                .iter()
                .map(|arg| Value::String((*arg).to_owned()))
                .collect(),
        ),
    );
    Value::Object(server)
}

fn patch_copilot(path: &Path, dry_run: bool) -> Result<WireAction, String> {
    let existed = path.exists();
    let mut root = read_existing(path, existed)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| format!("copilot: {} is not a JSON object", path.display()))?;
    let servers = server_table(object, "mcpServers", "copilot", path)?;

    let existing = servers.get("nika").cloned();
    let desired = copilot_server();
    let migrated = existing
        .as_ref()
        .is_some_and(crate::detect::is_stale_mcp_server);
    if existing.as_ref() == Some(&desired) {
        return Ok(WireAction::Current(format!("copilot: {}", path.display())));
    }
    servers.insert("nika".to_owned(), desired);
    if !dry_run {
        write_json(path, &root)?;
    }
    let label_path = format!("copilot: {}", path.display());
    if migrated {
        Ok(WireAction::Migrated(label_path))
    } else if existed {
        Ok(WireAction::Updated(label_path))
    } else {
        Ok(WireAction::Created(label_path))
    }
}

/// Amp's settings may be `.jsonc` (their docs allow it) — same lossless
/// contract as Zed: a file plain-JSON cannot round-trip gets the exact
/// snippet, byte-identical otherwise.
fn patch_amp(path: &Path, dry_run: bool) -> Result<WireAction, String> {
    let existed = path.exists();
    let mut root = if existed {
        let Some(value) = read_plain_json(path)? else {
            return Ok(WireAction::Manual(format!(
                "amp: {} {NOT_PLAIN_JSON} — add this under \
                 \"amp.mcpServers\" yourself:\n  \"nika\": {{ \
                 \"command\": \"nika\", \"args\": [\"mcp\"] }}",
                path.display()
            )));
        };
        value
    } else {
        Value::Object(Map::new())
    };
    let object = root
        .as_object_mut()
        .ok_or_else(|| format!("amp: {} is not a JSON object", path.display()))?;
    let servers = server_table(object, "amp.mcpServers", "amp", path)?;

    let existing = servers.get("nika").cloned();
    let desired = nika_server(false);
    let migrated = existing
        .as_ref()
        .is_some_and(crate::detect::is_stale_mcp_server);
    if existing.as_ref() == Some(&desired) {
        return Ok(WireAction::Current(format!("amp: {}", path.display())));
    }
    servers.insert("nika".to_owned(), desired);
    if !dry_run {
        write_json(path, &root)?;
    }
    let label_path = format!("amp: {}", path.display());
    if migrated {
        Ok(WireAction::Migrated(label_path))
    } else if existed {
        Ok(WireAction::Updated(label_path))
    } else {
        Ok(WireAction::Created(label_path))
    }
}

fn nika_server(include_type: bool) -> Value {
    let mut server = Map::new();
    if include_type {
        server.insert("type".to_owned(), Value::String("stdio".to_owned()));
    }
    server.insert("command".to_owned(), Value::String("nika".to_owned()));
    server.insert(
        "args".to_owned(),
        Value::Array(
            NIKA_MCP_ARGS
                .iter()
                .map(|arg| Value::String((*arg).to_owned()))
                .collect(),
        ),
    );
    Value::Object(server)
}

/// A JSON config as it is.
fn read_json(path: &Path) -> Result<Value, String> {
    let body = crate::detect::read_config(path)?;
    serde_json::from_str(&body).map_err(|e| format!("{}: malformed JSON: {e}", path.display()))
}

/// What a manual line says of a settings file that reads but is not
/// plain JSON. Its host allows comments, so that is the likely cause —
/// and a damaged file reads the same, so neither is asserted.
const NOT_PLAIN_JSON: &str = "is not plain JSON (it may hold comments, or be damaged)";

/// A settings file whose host reads JSONC (Zed, Amp): `None` when it
/// reads but is not plain JSON, which nika cannot rewrite without losing
/// what it could not parse. A file that cannot be read at all is an error
/// with its reason, never a `None`.
fn read_plain_json(path: &Path) -> Result<Option<Value>, String> {
    let text = crate::detect::read_config(path)?;
    Ok(serde_json::from_str(&text).ok())
}

/// The config as it is when it exists, else an empty object.
fn read_existing(path: &Path, existed: bool) -> Result<Value, String> {
    if existed {
        read_json(path)
    } else {
        Ok(Value::Object(Map::new()))
    }
}

/// The server table under `key`, created when absent. A value that is not
/// a JSON object is the user's, never ours to replace (an array of
/// servers was once erased to `{}`): refused, and the file is left as it is.
fn server_table<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    label: &str,
    path: &Path,
) -> Result<&'a mut Map<String, Value>, String> {
    object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            format!(
                "{label}: `{key}` in {} is not a JSON object; nika wire leaves it as it is",
                path.display()
            )
        })
}

/// The ONE write path for host configs (audit 2026-07-31): the body
/// lands in a same-dir temp file, is fsynced, then RENAMED over the
/// target — a crash mid-write can no longer truncate the user's whole
/// `~/.claude.json` in place. A NEW file gets 0600 (these configs grow
/// API keys); an existing file's mode is carried onto the temp file
/// BEFORE the rename so the swap preserves it.
///
/// A settings file that is a LINK (into a dotfiles folder under the
/// home, say) stays one: the temp file and the rename go beside the file
/// the link leads to ([`destination`]), so the link survives and what it
/// points at is what changes. Renaming over the link itself left a
/// regular file in its place, and the dotfiles copy stale. (A project
/// config's link is followed only to a regular file inside the project:
/// [`settled_path`].)
///
/// The temporary is a new file, created exclusively (it never opens an
/// entry already there, a link included), owner-only for a new config,
/// with an existing config's mode set through the open file; it is synced
/// (`sync_all`, as before) before the rename gives it the config's name
/// (`nika_fs::replace_file`), and removed on any failure.
fn write_atomic(path: &Path, body: &str) -> Result<(), String> {
    // The failure is the settings file's: the temporary file beside it is
    // nika's own business, and a name the person has never seen.
    let refuse = |e: std::io::Error| format!("{}: cannot write: {e}", path.display());
    let target = destination(path)?;
    if let Some(folder) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(folder).map_err(refuse)?;
    }
    nika_fs::replace_file(
        &target,
        body.as_bytes(),
        nika_fs::ReplaceMode::KeepOr(0o600),
        nika_fs::Durability::All,
    )
    .map_err(refuse)
}

/// Where the write to `path` goes: where a write to it lands now
/// ([`landing`], a real place: the file itself, or the one it links to,
/// however many links away; for a name not there yet, or a link whose
/// target is gone, that name in the real folder, so the write replaces the
/// dead link and never creates a file where it pointed). When a check holds
/// the config ([`Settled`]), that place must be the one the check found,
/// or nothing is written. The write goes to the place compared, never
/// resolved again. Whether a link may be followed at all is settled before
/// any writer runs ([`settled_path`]): only inside the folder that owns the
/// config, and at a project config's own name only to a regular file of the
/// project.
fn destination(path: &Path) -> Result<PathBuf, String> {
    let now = landing(path).map_err(|e| format!("{}: cannot write: {e}", path.display()))?;
    if let Some(then) = Settled::lands(path)
        && now != then
    {
        return Err(format!(
            "{}: changed since nika wire checked it: a write would now land at {}, not {}; \
             nothing written · run nika wire again",
            path.display(),
            now.display(),
            then.display()
        ));
    }
    Ok(now)
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let body = serde_json::to_string_pretty(value)
        .map_err(|e| format!("{}: cannot encode JSON: {e}", path.display()))?;
    write_atomic(path, &format!("{body}\n"))
}

fn render(actions: &[WireAction]) -> String {
    let mut lines = Vec::new();
    for action in actions {
        let line = match action {
            WireAction::Created(path) => format!("✔ created {path}"),
            WireAction::Updated(path) => format!("✔ updated {path}"),
            WireAction::Migrated(path) => {
                format!("✔ migrated {path} (`mcp serve --stdio` → `mcp`)")
            }
            WireAction::Current(path) => format!("· current {path}"),
            WireAction::Skipped(message) => format!("✖ skipped {message}"),
            WireAction::Manual(message) => format!("✋ manual {message}"),
        };
        lines.push(line);
    }
    lines.join("\n")
}

/// The `--dry-run` render: the SAME per-client outcomes as [`render`],
/// phrased as intent (« would ») under a header that says nothing was
/// written — a preview that reads like a receipt would be a lie (H7).
fn render_plan(actions: &[WireAction]) -> String {
    let mut lines = vec!["dry-run · plan only · nothing written".to_owned()];
    lines.extend(plan_lines(actions));
    lines.push("→ re-run without --dry-run to apply".to_owned());
    lines.join("\n")
}

/// A real run its preflight refused: the same plan, nothing
/// written, and the way on.
fn render_refused(actions: &[WireAction]) -> String {
    let mut lines = vec!["nothing written · a client config cannot be wired as it is".to_owned()];
    lines.extend(plan_lines(actions));
    lines.push(
        "→ fix the skipped config, or wire the others by name (`nika wire <client>`)".to_owned(),
    );
    lines.join("\n")
}

/// One line per planned outcome, phrased as intent.
fn plan_lines(actions: &[WireAction]) -> impl Iterator<Item = String> + '_ {
    actions.iter().map(|action| match action {
        WireAction::Created(path) => format!("+ would create {path}"),
        WireAction::Updated(path) => format!("+ would update {path}"),
        WireAction::Migrated(path) => {
            format!("+ would migrate {path} (`mcp serve --stdio` → `mcp`)")
        }
        WireAction::Current(path) => format!("· current {path}"),
        WireAction::Skipped(message) => format!("✖ skipped {message}"),
        WireAction::Manual(message) => format!("✋ manual {message}"),
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[cfg(unix)]
mod link_tests;
