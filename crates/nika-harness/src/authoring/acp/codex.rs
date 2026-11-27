// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The Codex ACP completion profile: codex-acp 1.13.1 and the codex 0.156.1 it bundles.
//!
//! Codex has no empty-tools switch, so the profile closes each tool-bearing surface BEFORE
//! the adapter starts, and proves it on the exact binary the adapter will run:
//!
//! - every tool-bearing feature off (the measured direct-profile list, all present in 0.156.1)
//!   and the non-feature tool surfaces off (web search, agents, plan, user input, skills);
//! - the plugins feature off, then every MCP server the configuration still defines disabled
//!   by name (an empty ACP `mcpServers` list keeps inherited servers, and an empty table
//!   merges instead of clearing);
//! - the same overrides read back on that binary before the spawn: `features list` must show
//!   each feature off and `mcp list` no enabled server, or no session opens;
//! - the adapter receives them as `CODEX_CONFIG` and runs exactly that binary (`CODEX_PATH`);
//! - the ACP mode `read-only` is applied and read back before the prompt: codex-acp sends its
//!   sandbox and approval with every turn from that mode — the fresh scratch directory is the
//!   only writable root, there is no network, and every approval request reaches the client,
//!   which the completion profile always denies.
//!
//! The proof reads the layers the adapter's Codex home and this machine hold (the user's, the
//! system's and the managed ones, which [`config_stamp`] also watches), listed from Nika's own
//! working directory. A project's own `.codex` configuration belongs to a session's directory,
//! not to the seat: the proof neither reads nor watches it. codex-acp trusts every session root,
//! so Codex would apply it, and a session that would see one is refused right before its
//! process would start ([`project_config`]), whatever the session's kind and wherever its
//! directory lies, a scratch the engine created included.
//!
//! An MCP tool approval reaches the client only through Codex's MCP elicitation route
//! ([`APPROVAL_FEATURE`]). With that feature off, Codex asks through `request_user_input`
//! instead, which codex-acp answers itself with no answer (Nika advertises no form
//! elicitation), and Codex reads that as the user cancelling the call. A conversation keeps the
//! route on, read back like every other feature, so Nika answers each ask; the completion
//! profile, which mounts no MCP server, keeps it off with the rest.
//!
//! `apply_patch` cannot be unregistered on this pair (the model catalogue registers it
//! whenever the turn has an environment); under `read-only` it can write only inside the
//! scratch, and any tool beat refuses the whole answer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// The adapter identity this profile was built and measured for.
pub(crate) const NAME: &str = "@agentclientprotocol/codex-acp";
/// The exact adapter version (it pins the bundled codex 0.156.1).
pub(crate) const VERSION: &str = "1.13.1";
/// The ACP mode applied and read back before the prompt.
pub(crate) const MODE: &str = "read-only";
/// The variable a conversation's Codex reads Nika's tool server bearer from
/// (`bearer_token_env_var`): the bearer rides the adapter's environment only, never the
/// configuration, a command line or a read-back.
pub(crate) const BEARER_ENV: &str = "NIKA_MCP_BEARER";
/// The Codex feature that routes an MCP tool approval to the client as an MCP elicitation,
/// which codex-acp turns into `session/request_permission` (codex 0.156.1
/// `request_mcp_tool_user_approval`). It adds no tool: a conversation keeps it on.
pub(crate) const APPROVAL_FEATURE: &str = "tool_call_mcp_elicitation";
/// The variable and value that keep codex-acp from suggesting a next prompt. Read in the 2.12.0
/// source (`PromptSuggestionGenerator.ts`): after each completed turn, for a client whose
/// `initialize` declares the Air extension (Nika's agent and one-shot sessions do), the adapter
/// samples one more model response on a fork of the session unless this variable reads
/// `false`. 1.13.1 has no such sampling and does not read it. Every session of the seat
/// carries it.
pub(crate) const SUGGESTIONS_ENV: (&str, &str) = ("CODEX_ACP_PROMPT_SUGGESTIONS", "false");

/// The names a codex-acp release reports as its `agentInfo.name` on `initialize` (the measured
/// boundary row of the `codex` seat): the maintained package and the deprecated one.
pub(crate) fn reports_as_codex_acp(name: &str) -> bool {
    nika_types::access::HarnessBoundary::for_agent(name).is_some_and(|row| row.seat == "codex")
}

/// Whether the program `command` is codex-acp, by its file name (`codex-acp`, with or without
/// an extension such as a Windows launcher's), wherever it lives.
pub(crate) fn runs_codex_acp(command: &str) -> bool {
    (Path::new(command).file_stem()).is_some_and(|stem| stem.eq_ignore_ascii_case("codex-acp"))
}

/// A `.codex` entry a session would see, named as found. `unread` is the reason an entry could
/// not be looked at: it counts as present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The entry: the `.codex` itself, or the `config.toml` a `.codex` directory holds.
    pub(crate) path: PathBuf,
    /// Why the entry could not be looked at, when it could not.
    pub(crate) unread: Option<std::io::ErrorKind>,
}

impl Entry {
    /// The entry as a refusal names it.
    fn named(&self) -> String {
        match self.unread {
            None => format!("`{}`", self.path.display()),
            Some(kind) => format!("`{}` (not readable here: {kind})", self.path.display()),
        }
    }
}

/// The configuration the adapter starts with, once `servers` (the MCP servers the user
/// configuration defines) are known: every tool-bearing surface off.
pub(crate) fn config(servers: &[String]) -> Value {
    let features: Map<String, Value> = crate::infer::tool_free::DISABLED_FEATURES
        .iter()
        .map(|feature| ((*feature).to_owned(), Value::Bool(false)))
        .collect();
    let mcp: Map<String, Value> = servers
        .iter()
        .map(|name| (name.clone(), json!({"enabled": false})))
        .collect();
    json!({
        "features": features,
        "web_search": "disabled",
        "agents": {"enabled": false},
        "tools": {
            "update_plan": {"enabled": false},
            "experimental_request_user_input": {"enabled": false}
        },
        "skills": {"include_instructions": false},
        "cloud": {"skills": {"enabled": false}},
        "mcp_servers": mcp
    })
}

/// [`config`] for a conversation: Nika's tool server `server` mounted as `name`, the one MCP
/// server the profile enables, and Codex's approval route to Nika ([`APPROVAL_FEATURE`]) on.
///
/// # Errors
/// The configuration already defines a server of that name: mounting Nika's under it would
/// merge both definitions.
pub(crate) fn conversation_config(
    servers: &[String],
    name: &str,
    server: Value,
) -> Result<Value, String> {
    if servers.iter().any(|defined| defined == name) {
        return Err(format!(
            "the Codex configuration defines an MCP server named `{name}`, the name Nika's \
             conversation tool server mounts under; rename that server"
        ));
    }
    let mut config = config(servers);
    if let Some(mcp) = config.get_mut("mcp_servers").and_then(Value::as_object_mut) {
        mcp.insert(name.to_owned(), server);
    }
    if let Some(features) = config.get_mut("features").and_then(Value::as_object_mut) {
        features.insert(APPROVAL_FEATURE.to_owned(), Value::Bool(true));
    }
    Ok(config)
}

/// The `-c key=value` overrides equivalent to [`config`] — the SAME object flattened, so the
/// read-back judges exactly what the adapter receives.
pub(crate) fn overrides(config: &Value) -> Vec<String> {
    let mut out = Vec::new();
    flatten("", config, &mut out);
    out
}

fn flatten(prefix: &str, value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let key = toml_key(key);
                let path = if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&path, child, out);
            }
        }
        Value::String(text) => out.push(format!("{prefix}={}", Value::String(text.clone()))),
        other => out.push(format!("{prefix}={other}")),
    }
}

/// A bare TOML key when it is one, else a quoted one.
fn toml_key(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        key.to_owned()
    } else {
        Value::String(key.to_owned()).to_string()
    }
}

/// The MCP servers the configuration defines (plugins already off), from `mcp list --json`.
///
/// # Errors
/// The listing is not the documented JSON array of `{name, …}` objects.
pub(crate) fn defined_servers(listing: &[u8]) -> Result<Vec<String>, String> {
    let rows: Vec<Value> = serde_json::from_slice(listing)
        .map_err(|_| "codex mcp list did not answer a JSON array".to_owned())?;
    rows.iter()
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "codex mcp list answered a server without a name".to_owned())
        })
        .collect()
}

/// Every MCP server still listed under the profile must be disabled.
///
/// # Errors
/// The servers still enabled, by name.
pub(crate) fn judge_servers(listing: &[u8]) -> Result<(), String> {
    let rows: Vec<Value> = serde_json::from_slice(listing)
        .map_err(|_| "codex mcp list did not answer a JSON array".to_owned())?;
    let enabled: Vec<&str> = rows
        .iter()
        .filter(|row| row.get("enabled").and_then(Value::as_bool) != Some(false))
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unnamed)")
        })
        .collect();
    if enabled.is_empty() {
        return Ok(());
    }
    Err(format!(
        "MCP servers still enabled under the profile: {}",
        enabled.join(" · ")
    ))
}

/// Under the conversation profile exactly one MCP server is enabled: Nika's, named `name`.
///
/// # Errors
/// The servers enabled instead, by name.
pub(crate) fn judge_conversation(listing: &[u8], name: &str) -> Result<(), String> {
    let rows: Vec<Value> = serde_json::from_slice(listing)
        .map_err(|_| "codex mcp list did not answer a JSON array".to_owned())?;
    let enabled: Vec<&str> = (rows.iter())
        .filter(|row| row.get("enabled").and_then(Value::as_bool) != Some(false))
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unnamed)")
        })
        .collect();
    if enabled == [name] {
        return Ok(());
    }
    Err(format!(
        "the conversation profile enables exactly Nika's MCP server `{name}`; enabled: {}",
        if enabled.is_empty() {
            "none".to_owned()
        } else {
            enabled.join(" · ")
        }
    ))
}

/// What a proven profile was proven against: the Codex home the adapter resolves
/// (`CODEX_HOME`, else `$HOME/.codex`) and a stat (device, inode, size and modification time,
/// or its absence) of each configuration layer that can add or enable an MCP server
/// ([`layer_files`]). The profile disables the MCP servers those layers defined at proof time,
/// so a kept proof holds only while this stamp is unchanged; another home, or a layer written,
/// replaced or removed since, is proven again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigStamp {
    home: Option<PathBuf>,
    layers: Vec<(PathBuf, Option<FileStamp>)>,
}

/// Codex's system configuration directory on Unix.
const SYSTEM_DIR: &str = "/etc/codex";

/// Where macOS keeps the managed preferences a device profile pushes.
const MANAGED_PREFERENCES: &str = "/Library/Managed Preferences";

/// The managed preferences file of Codex's `com.openai.codex` domain.
const PREFERENCES_FILE: &str = "com.openai.codex.plist";

/// The identity and the last write of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileStamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        let (device, inode) = {
            use std::os::unix::fs::MetadataExt as _;
            (meta.dev(), meta.ino())
        };
        #[cfg(not(unix))]
        let (device, inode) = (0, 0);
        Self {
            device,
            inode,
            size: meta.len(),
            modified: meta.modified().ok(),
        }
    }
}

/// The [`ConfigStamp`] of the adapter environment `env` (the composed child environment:
/// what the adapter's codex itself resolves), on this machine. An unreadable layer reads as
/// absent.
pub(crate) fn config_stamp(env: &BTreeMap<String, String>) -> ConfigStamp {
    let preferences = cfg!(target_os = "macos").then(|| Path::new(MANAGED_PREFERENCES));
    config_stamp_in(env, Path::new(SYSTEM_DIR), preferences)
}

/// [`config_stamp`] with the machine's roots given: Codex's `system` directory and the
/// managed `preferences` directory, if this system has one.
pub(crate) fn config_stamp_in(
    env: &BTreeMap<String, String>,
    system: &Path,
    preferences: Option<&Path>,
) -> ConfigStamp {
    let home = codex_home(env);
    let layers = layer_files(home.as_deref(), env, system, preferences)
        .into_iter()
        .map(|path| {
            let file = std::fs::metadata(&path)
                .ok()
                .map(|meta| FileStamp::of(&meta));
            (path, file)
        })
        .collect();
    ConfigStamp { home, layers }
}

/// The Codex home the adapter environment `env` resolves: `CODEX_HOME`, else `$HOME/.codex`.
fn codex_home(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let set = |name: &str| env.get(name).filter(|value| !value.is_empty());
    set("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| set("HOME").map(|home| Path::new(home).join(".codex")))
}

/// The project configuration a Codex session rooted at `cwd` would see. codex-acp sets every
/// session root trusted (`projects.<root>.trust_level`, over any `projects` key the client's
/// configuration holds), and Codex applies the `.codex` configuration of a trusted project:
/// the one in the session's directory and the one in each parent up to the repository root.
/// `cwd` is judged as an absolute path without `.` components ([`project_config`] refuses one
/// that holds `..`), so no directory is looked at twice.
///
/// The repository root is the first directory whose `.git` resolves (a directory, a linked
/// worktree's file, or a link to either). A `.git` that does not resolve, such as a dangling
/// link, does not end the walk: with no `.git` that resolves, the walk goes up to the outermost
/// directory that holds any `.git` entry, and with none at all it looks at the directory alone.
/// Each choice walks at least as far as Codex could.
///
/// Each `.codex` entry is named as found, never followed: a link counts, dangling or not, and a
/// `.codex` directory that holds a `config.toml` is named by that file. A lookup that fails for
/// another reason than an absent entry (a directory this process may not search) counts as an
/// entry, named with that reason. The seat's own Codex home (`home`) is the user's
/// configuration, which the seat applies on every session anyway: it is not counted, when the
/// entry is the home as written, when an entry that is not a link sits where the home resolves
/// to, or when the entry bears the home's name in the directory the home's parent resolves to.
pub(crate) fn project_entries(cwd: &Path, home: Option<&Path>) -> Vec<Entry> {
    // `absolute` drops `.` parts, and each ancestor is one component up: no directory twice.
    let cwd = std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let chain: Vec<&Path> = cwd.ancestors().collect();
    let git = |dir: &Path| dir.join(".git");
    let reached = (chain
        .iter()
        .position(|dir| std::fs::metadata(git(dir)).is_ok()))
    .or_else(|| (chain.iter()).rposition(|dir| std::fs::symlink_metadata(git(dir)).is_ok()))
    .map_or(1, |root| root + 1);
    (chain.into_iter().take(reached))
        .filter_map(|dir| entry_in(dir, home))
        .collect()
}

/// The `.codex` entry `dir` holds, as [`project_entries`] counts it.
fn entry_in(dir: &Path, home: Option<&Path>) -> Option<Entry> {
    use std::io::ErrorKind;
    let entry = dir.join(".codex");
    let found = match std::fs::symlink_metadata(&entry) {
        Ok(found) => found,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return None;
        }
        Err(e) => {
            return Some(Entry {
                path: entry,
                unread: Some(e.kind()),
            });
        }
    };
    let link = found.file_type().is_symlink();
    if home.is_some_and(|home| is_home(dir, &entry, link, home)) {
        return None;
    }
    let config = entry.join("config.toml");
    let path = if found.is_dir() && std::fs::symlink_metadata(&config).is_ok() {
        config
    } else {
        entry
    };
    Some(Entry { path, unread: None })
}

/// Whether the entry `entry` of `dir` is the Codex home `home`: the same path as written; an
/// entry that is not a link (`link`) at the path the home resolves to (a home that is a link
/// into a checkout); or the same name in the directory `home`'s parent resolves to. An entry
/// that is a link is never followed, so a `.codex` link that leads to the home still counts.
fn is_home(dir: &Path, entry: &Path, link: bool, home: &Path) -> bool {
    let resolved = |path: &Path| std::fs::canonicalize(path).ok();
    home == entry
        || (!link && resolved(home).is_some_and(|home| resolved(entry) == Some(home)))
        || (home.file_name() == entry.file_name()
            && (home.parent())
                .and_then(resolved)
                .is_some_and(|parent| resolved(dir) == Some(parent)))
}

/// [`project_entries`] for the adapter environment `env`, judged: a session that would see a
/// project configuration does not open, and neither does one whose directory holds `..`,
/// which the walk could not judge as Codex reads it. The Codex home is not counted only when
/// the environment names it as an absolute path.
///
/// # Errors
/// The entries the session would see, each named, and that Codex would apply them; or the
/// directory holds `..`.
pub(crate) fn project_config(cwd: &Path, env: &BTreeMap<String, String>) -> Result<(), String> {
    let absolute = std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    if (absolute.components()).any(|part| part == std::path::Component::ParentDir) {
        return Err(format!(
            "the session directory `{}` is named with `..`, so the project configuration Codex \
             would apply to it cannot be judged; no session was opened; no fallback (name the \
             directory without `..`; a temporary directory takes its name from `TMPDIR`)",
            cwd.display()
        ));
    }
    // A home that is not absolute names no directory of its own: it would resolve against the
    // working directory, which is the project's for a task. Nothing is exempt for it.
    let home = codex_home(env).filter(|home| home.is_absolute());
    let found = project_entries(cwd, home.as_deref());
    if found.is_empty() {
        return Ok(());
    }
    let named: Vec<String> = found.iter().map(Entry::named).collect();
    Err(format!(
        "Codex would apply the project configuration {} to this session: codex-acp trusts the \
         session's directory, and Codex reads the `.codex` entry of that directory and of each \
         parent up to the repository root; no session was opened; no fallback (move the entry \
         out of the project, or run the task from a directory that has none)",
        named.join(" · ")
    ))
}

/// The configuration layers that can add or enable an MCP server, besides the profile's own
/// `-c` overrides, as Codex documents them: its configuration precedence
/// (<https://learn.chatgpt.com/docs/config-file/config-basic#configuration-precedence>) and its
/// managed configuration (<https://learn.chatgpt.com/docs/enterprise/managed-configuration>):
/// - the user's `config.toml` in the Codex `home`;
/// - the system `config.toml` (`/etc/codex/config.toml` on Unix);
/// - the managed defaults, `managed_config.toml` (`/etc/codex/managed_config.toml` on Unix,
///   `managed_config.toml` in the Codex home elsewhere), which override the user's file;
/// - `requirements.toml` (`/etc/codex/requirements.toml` on Unix,
///   `%ProgramData%\OpenAI\Codex\requirements.toml` on Windows), whose `mcp_servers`
///   allowlist decides which defined servers the client enables;
/// - on macOS, the managed preferences of the `com.openai.codex` domain (`config_toml_base64`,
///   `requirements_toml_base64`), in the device-wide file and in each user's.
///
/// Not stamped: a profile file (`<home>/<name>.config.toml`) loads only under `--profile`,
/// which the adapter's argv never passes, and since Codex 0.134.0 no `config.toml` key selects
/// one (<https://learn.chatgpt.com/docs/config-file/config-advanced#profiles>); cloud-managed
/// defaults and requirements come from a signed cache whose place on disk the documentation
/// does not give; a trusted project's `.codex/config.toml` belongs to a session's directory,
/// not to the seat, and a session that would see one does not open ([`project_config`]).
fn layer_files(
    home: Option<&Path>,
    env: &BTreeMap<String, String>,
    system: &Path,
    preferences: Option<&Path>,
) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = home
        .map(|home| home.join("config.toml"))
        .into_iter()
        .collect();
    if cfg!(unix) {
        let names = ["config.toml", "managed_config.toml", "requirements.toml"];
        files.extend(names.map(|name| system.join(name)));
    } else {
        files.extend(home.map(|home| home.join("managed_config.toml")));
        let data = env.get("PROGRAMDATA").filter(|value| !value.is_empty());
        files.extend(data.map(|data| Path::new(data).join("OpenAI/Codex/requirements.toml")));
    }
    if let Some(preferences) = preferences {
        files.push(preferences.join(PREFERENCES_FILE));
        let mut users: Vec<PathBuf> = (std::fs::read_dir(preferences).into_iter().flatten())
            .filter_map(Result::ok)
            .map(|entry| entry.path().join(PREFERENCES_FILE))
            .filter(|path| path.is_file())
            .collect();
        users.sort();
        files.extend(users);
    }
    files
}

/// The codex binary the adapter bundles, next to its own entry point in the npm package
/// (`<package>/dist/index.js` → `<package>/node_modules/@openai/codex/bin/codex.js`). The
/// adapter command is resolved on the child's PATH, symlinks followed.
///
/// # Errors
/// The adapter or its bundled codex is not where the measured package puts them.
pub(crate) fn bundled_codex(
    command: &str,
    env: &BTreeMap<String, String>,
) -> Result<PathBuf, String> {
    let entry = resolve(command, env.get("PATH").map(String::as_str))
        .ok_or_else(|| format!("`{command}` is not on the adapter PATH"))?;
    let entry = std::fs::canonicalize(&entry)
        .map_err(|error| format!("cannot resolve `{}`: {error}", entry.display()))?;
    let package = entry
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("`{}` sits in no package", entry.display()))?;
    let codex = package
        .join("node_modules")
        .join("@openai")
        .join("codex")
        .join("bin")
        .join("codex.js");
    if codex.is_file() {
        Ok(codex)
    } else {
        Err(format!(
            "codex-acp bundles no codex at `{}` (the measured npm layout)",
            codex.display()
        ))
    }
}

fn resolve(command: &str, path: Option<&str>) -> Option<PathBuf> {
    let direct = Path::new(command);
    if direct.components().count() > 1 {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    std::env::split_paths(path?)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod project_tests;
