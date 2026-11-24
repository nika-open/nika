// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

use super::*;
use serde_json::json;

/// #330 · opencode: its OWN shape (`mcp.nika` · type local · argv in
/// `command`) — created project-local, idempotent on re-run, other
/// servers preserved.
#[test]
fn opencode_project_config_created_and_idempotent() {
    let dir = temp_dir("opencode");
    let path = dir.join("opencode.json");
    let first = patch_opencode(&path, false).expect("create");
    assert!(matches!(first, WireAction::Created(_)), "{first:?}");
    let body: Value = read_json(&path).expect("json");
    assert_eq!(body["mcp"]["nika"]["type"], "local");
    assert_eq!(body["mcp"]["nika"]["command"][0], "nika");
    assert_eq!(body["mcp"]["nika"]["command"][1], "mcp");
    assert_eq!(body["mcp"]["nika"]["enabled"], true);
    let second = patch_opencode(&path, false).expect("re-run");
    assert!(matches!(second, WireAction::Current(_)), "{second:?}");
}

#[test]
fn opencode_preserves_other_servers_and_repairs_nika() {
    let dir = temp_dir("opencode-merge");
    let path = dir.join("opencode.json");
    std::fs::write(
            &path,
            r#"{"mcp":{"other":{"type":"remote"},"nika":{"type":"local","command":["nika","mcp","serve"]}},"theme":"dark"}"#,
        )
        .expect("seed");
    let action = patch_opencode(&path, false).expect("repair");
    assert!(matches!(action, WireAction::Updated(_)), "{action:?}");
    let body: Value = read_json(&path).expect("json");
    assert_eq!(
        body["mcp"]["other"]["type"], "remote",
        "unrelated server kept"
    );
    assert_eq!(body["theme"], "dark", "unrelated key kept");
    assert_eq!(body["mcp"]["nika"]["command"][1], "mcp", "argv repaired");
}

/// #330 · hermes: YAML — the Zed contract (create-if-missing ·
/// current-detect · Manual otherwise, file byte-identical).
#[test]
fn hermes_yaml_creates_detects_and_never_rewrites() {
    let dir = temp_dir("hermes");
    let path = dir.join("config.yaml");
    let first = patch_hermes(&path, false).expect("create");
    assert!(matches!(first, WireAction::Created(_)), "{first:?}");
    let second = patch_hermes(&path, false).expect("re-run");
    assert!(matches!(second, WireAction::Current(_)), "{second:?}");
    // A pre-existing file WITHOUT our entry: snippet handed back,
    // bytes untouched (comments/anchors are the operator's).
    let foreign = "# my hermes\nmodel: hermes-4\nmcp_servers:\n  other: { command: x }\n";
    std::fs::write(&path, foreign).expect("seed");
    let third = patch_hermes(&path, false).expect("manual");
    let WireAction::Manual(msg) = &third else {
        unreachable!("expected Manual, got {third:?}");
    };
    assert!(msg.contains("mcp_servers:") && msg.contains("command: nika"));
    let after = std::fs::read_to_string(&path).expect("read");
    assert_eq!(after, foreign, "the operator file stays byte-identical");
}

#[test]
fn vscode_config_uses_stdio_and_mcp_arg() {
    let dir = temp_dir("vscode");
    let path = dir.join("vscode-mcp.json");
    let action = patch_vscode(&path, false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)));
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["servers"]["nika"]["type"], "stdio");
    assert_eq!(doc["servers"]["nika"]["command"], "nika");
    assert_eq!(doc["servers"]["nika"]["args"], json!(["mcp"]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cursor_like_config_preserves_other_servers_and_migrates_stale_args() {
    let dir = temp_dir("cursor");
    let path = dir.join("mcp.json");
    std::fs::write(
        &path,
        r#"{
  "mcpServers": {
    "github": { "command": "gh", "args": ["mcp"] },
    "nika": { "command": "nika", "args": ["mcp", "serve", "--stdio"] }
  }
}
"#,
    )
    .expect("fixture");

    let action = patch_cursor_like(&path, "mcpServers", "cursor", false, false).expect("wire");
    assert!(matches!(action, WireAction::Migrated(_)));
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["github"]["command"], "gh");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn codex_config_toml_created_with_mcp_servers_table() {
    let dir = temp_dir("codex-create");
    let path = dir.join("config.toml");
    let action = patch_toml_mcp(&path, "codex", false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)));
    let body = std::fs::read_to_string(&path).expect("read");
    let doc: toml_edit::DocumentMut = body.parse().expect("toml");
    assert_eq!(doc["mcp_servers"]["nika"]["command"].as_str(), Some("nika"));
    let args: Vec<&str> = doc["mcp_servers"]["nika"]["args"]
        .as_array()
        .expect("args array")
        .iter()
        .filter_map(toml_edit::Value::as_str)
        .collect();
    assert_eq!(args, ["mcp"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn codex_config_preserves_other_tables_comments_and_is_idempotent() {
    let dir = temp_dir("codex-preserve");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        r#"# my codex setup
model = "gpt-5.2"

[projects."/Users/me/repo"]
trust_level = "trusted"

[mcp_servers.github]
command = "gh"
args = ["mcp"]
"#,
    )
    .expect("fixture");

    let action = patch_toml_mcp(&path, "codex", false).expect("wire");
    assert!(matches!(action, WireAction::Updated(_)));
    let body = std::fs::read_to_string(&path).expect("read");
    assert!(body.contains("# my codex setup"), "comment preserved");
    assert!(body.contains("trust_level = \"trusted\""));
    assert!(body.contains("[mcp_servers.github]"));

    let again = patch_toml_mcp(&path, "codex", false).expect("wire twice");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn codex_stale_serve_args_are_migrated() {
    let dir = temp_dir("codex-migrate");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        "[mcp_servers.nika]\ncommand = \"nika\"\nargs = [\"mcp\", \"serve\", \"--stdio\"]\n",
    )
    .expect("fixture");

    let action = patch_toml_mcp(&path, "codex", false).expect("wire");
    assert!(matches!(action, WireAction::Migrated(_)));
    let body = std::fs::read_to_string(&path).expect("read");
    assert!(body.contains("args = [\"mcp\"]"));
    let _ = std::fs::remove_dir_all(dir);
}

/// Client-doors W1.2 · grok: the shared `[mcp_servers.nika]` TOML shape
/// at `~/.grok/config.toml` — created, comments preserved, idempotent.
#[test]
fn grok_config_toml_created_preserves_comments_and_is_idempotent() {
    let dir = temp_dir("grok");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        "# my grok setup\nmodel = \"grok-4-3\"\n\n[compat.claude]\nskills = true\n",
    )
    .expect("fixture");

    let action = patch_toml_mcp(&path, "grok", false).expect("wire");
    assert!(matches!(action, WireAction::Updated(_)), "{action:?}");
    let body = std::fs::read_to_string(&path).expect("read");
    assert!(body.contains("# my grok setup"), "comment preserved");
    assert!(body.contains("[compat.claude]"), "unrelated table kept");
    let doc: toml_edit::DocumentMut = body.parse().expect("toml");
    assert_eq!(doc["mcp_servers"]["nika"]["command"].as_str(), Some("nika"));

    let again = patch_toml_mcp(&path, "grok", false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(dir);
}

/// Wave-3 · copilot: the desired matches THEIR writer byte-shape
/// (tools `["*"]` + type local) — a copilot-added entry reads Current
/// on first contact, never churns. Empirical fixture = the exact
/// bytes `copilot mcp add nika -- nika mcp` wrote (1.0.76 · live).
#[test]
fn copilot_matches_their_own_writer_and_is_idempotent() {
    let home = temp_dir("copilot");
    let path = home.join(".copilot").join("mcp-config.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dotdir");
    std::fs::write(
        &path,
        r#"{"mcpServers":{"nika":{"tools":["*"],"type":"local","command":"nika","args":["mcp"]}}}"#,
    )
    .expect("their writer's bytes");

    let action = patch_copilot(&path, false).expect("wire");
    assert!(
        matches!(action, WireAction::Current(_)),
        "their entry must read Current, got {action:?}"
    );

    std::fs::remove_file(&path).expect("reset");
    let created = patch_copilot(&path, false).expect("create");
    assert!(matches!(created, WireAction::Created(_)));
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["type"], "local");
    assert_eq!(doc["mcpServers"]["nika"]["tools"], json!(["*"]));
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));
    let _ = std::fs::remove_dir_all(home);
}

/// Wave-3 · amp: the LITERAL dotted key "amp.mcpServers" at the
/// settings root (their writer's shape) · unrelated settings kept ·
/// JSONC gets the snippet, byte-identical (the Zed contract).
#[test]
fn amp_dotted_key_preserves_settings_and_respects_jsonc() {
    let home = temp_dir("amp");
    let path = home.join(".config").join("amp").join("settings.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(
        &path,
        r#"{"amp.notifications.enabled":true,"amp.mcpServers":{"other":{"command":"x"}}}"#,
    )
    .expect("seed");

    let action = patch_amp(&path, false).expect("wire");
    assert!(matches!(action, WireAction::Updated(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["amp.notifications.enabled"], true, "unrelated kept");
    assert_eq!(doc["amp.mcpServers"]["other"]["command"], "x");
    assert_eq!(doc["amp.mcpServers"]["nika"]["command"], "nika");

    let again = patch_amp(&path, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));

    let jsonc = "{\n  // amp settings\n  \"amp.anthropic.thinking\": true\n}\n";
    std::fs::write(&path, jsonc).expect("jsonc seed");
    let manual = patch_amp(&path, false).expect("manual");
    assert!(
        matches!(&manual, WireAction::Manual(m) if m.contains("amp.mcpServers")),
        "{manual:?}"
    );
    let after = std::fs::read_to_string(&path).expect("read");
    assert_eq!(after, jsonc, "JSONC byte-identical");
    let _ = std::fs::remove_dir_all(home);
}

/// The registry law: `wire all` expands to every concrete client —
/// a variant added to the enum can never be silently dropped. The
/// count is PINNED (anti-drift: the list moved 15→17→19→21): adding
/// a client means updating this pin DELIBERATELY. `All`/`Detected`
/// are doors, never expansion results.
#[test]
fn all_expands_to_every_concrete_target() {
    let expanded = expand_target(WireTarget::All);
    assert_eq!(
        expanded.len(),
        21,
        "the concrete-client pin moved — deliberate?"
    );
    assert_eq!(
        expanded.len(),
        WireTarget::value_variants().len() - 2,
        "every variant minus the two doors"
    );
    assert!(!expanded.contains(&WireTarget::All));
    assert!(!expanded.contains(&WireTarget::Detected));
}

/// Client-doors W3 · kimi: the user-level `~/.kimi-code/mcp.json`
/// (`mcpServers` map · their customization/mcp.md) — created, then
/// idempotent, other servers preserved.
#[test]
fn kimi_mcp_json_created_and_idempotent() {
    let home = temp_dir("kimi");
    let path = home.join(".kimi-code").join("mcp.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dotdir");
    std::fs::write(&path, r#"{"mcpServers":{"other":{"command":"x"}}}"#).expect("seed");

    let action = patch_cursor_like(&path, "mcpServers", "kimi", false, false).expect("wire");
    assert!(matches!(action, WireAction::Updated(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["command"], "nika");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));
    assert_eq!(
        doc["mcpServers"]["other"]["command"], "x",
        "other server kept"
    );

    let again = patch_cursor_like(&path, "mcpServers", "kimi", false, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(home);
}

/// Client-doors W1.2 · antigravity: the standalone `mcp_config.json`
/// under `~/.gemini/config/` (`mcpServers` root · gcli-migration doc) —
/// nested dirs created, then idempotent.
#[test]
fn antigravity_mcp_config_created_nested_and_idempotent() {
    let home = temp_dir("antigravity");
    let path = home.join(".gemini").join("config").join("mcp_config.json");

    let action = patch_cursor_like(&path, "mcpServers", "antigravity", false, false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["command"], "nika");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));

    let again =
        patch_cursor_like(&path, "mcpServers", "antigravity", false, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn current_config_is_idempotent() {
    let dir = temp_dir("idempotent");
    let path = dir.join("mcp.json");
    let _ = patch_cursor_like(&path, "mcpServers", "cursor", false, false).expect("wire");
    let action = patch_cursor_like(&path, "mcpServers", "cursor", false, false).expect("wire");
    assert!(matches!(action, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zed_config_created_under_context_servers() {
    let dir = temp_dir("zed-create");
    let path = dir.join("settings.json");
    let action = patch_zed(&path, false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)));
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["context_servers"]["nika"]["command"], "nika");
    assert_eq!(doc["context_servers"]["nika"]["args"], json!(["mcp"]));
    assert_eq!(doc["context_servers"]["nika"]["env"], json!({}));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zed_jsonc_settings_are_never_destroyed() {
    // Zed's settings.json is JSONC — the DEFAULT file ships with comments.
    // A naive parse-rewrite would strip them; the contract is: never
    // rewrite a file we cannot round-trip losslessly — hand the user the
    // exact snippet instead.
    let dir = temp_dir("zed-jsonc");
    let path = dir.join("settings.json");
    let jsonc = "{\n  // Zed settings\n  \"theme\": \"One Dark\"\n}\n";
    std::fs::write(&path, jsonc).expect("fixture");

    let action = patch_zed(&path, false).expect("wire");
    assert!(
        matches!(
            &action,
            WireAction::Manual(message)
                if message.contains("context_servers")
                    && message.contains("\"command\": \"nika\"")
        ),
        "expected Manual with the snippet, got {action:?}"
    );
    let body = std::fs::read_to_string(&path).expect("read");
    assert_eq!(
        body, jsonc,
        "JSONC file must be byte-identical (never destroyed)"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zed_preserves_other_context_servers_and_migrates_stale() {
    let dir = temp_dir("zed-migrate");
    let path = dir.join("settings.json");
    std::fs::write(
        &path,
        r#"{
  "theme": "One Dark",
  "context_servers": {
    "github": { "command": "gh", "args": ["mcp"] },
    "nika": { "command": "nika", "args": ["mcp", "serve", "--stdio"] }
  }
}
"#,
    )
    .expect("fixture");

    let action = patch_zed(&path, false).expect("wire");
    assert!(matches!(action, WireAction::Migrated(_)));
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["theme"], "One Dark", "unrelated settings preserved");
    assert_eq!(doc["context_servers"]["github"]["command"], "gh");
    assert_eq!(doc["context_servers"]["nika"]["args"], json!(["mcp"]));

    let again = patch_zed(&path, false).expect("wire twice");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(dir);
}

/// #384 · gemini: `mcpServers` lives inside the SHARED settings.json —
/// every unrelated setting survives, stale argv migrates.
#[test]
fn gemini_settings_preserves_unrelated_keys_and_migrates_stale() {
    let dir = temp_dir("gemini");
    let path = dir.join("settings.json");
    std::fs::write(
        &path,
        r#"{
  "theme": "GitHub",
  "autoAccept": false,
  "mcpServers": {
    "context7": { "command": "npx", "args": ["-y", "@upstash/context7-mcp"] },
    "nika": { "command": "nika", "args": ["mcp", "serve", "--stdio"] }
  }
}
"#,
    )
    .expect("fixture");

    let action = patch_cursor_like(&path, "mcpServers", "gemini", false, false).expect("wire");
    assert!(matches!(action, WireAction::Migrated(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["theme"], "GitHub", "unrelated settings preserved");
    assert_eq!(doc["autoAccept"], false, "unrelated settings preserved");
    assert_eq!(doc["mcpServers"]["context7"]["command"], "npx");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));

    let again = patch_cursor_like(&path, "mcpServers", "gemini", false, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(dir);
}

/// #384 · lmstudio: the documented dir wins, the cache dir
/// (lmstudio-bug-tracker#1371) is honoured only when it is the ONLY
/// app-created root, and a bare home falls back to the documented
/// default.
#[test]
fn lmstudio_path_resolution_truth_table() {
    let home = temp_dir("lmstudio-home");

    // Bare home — documented default (created on write).
    assert_eq!(
        lmstudio_mcp_path_from(&home),
        home.join(".lmstudio").join("mcp.json")
    );

    // Only the cache root exists — the app lives there, follow it.
    std::fs::create_dir_all(home.join(".cache").join("lm-studio")).expect("cache root");
    assert_eq!(
        lmstudio_mcp_path_from(&home),
        home.join(".cache").join("lm-studio").join("mcp.json")
    );

    // Documented root exists — it wins even beside the cache root.
    std::fs::create_dir_all(home.join(".lmstudio")).expect("documented root");
    assert_eq!(
        lmstudio_mcp_path_from(&home),
        home.join(".lmstudio").join("mcp.json")
    );
    let _ = std::fs::remove_dir_all(home);
}

/// #384 · lmstudio: dedicated `mcp.json` (Cursor-style `mcpServers`) —
/// created then idempotent at the resolved path.
#[test]
fn lmstudio_config_created_and_idempotent() {
    let home = temp_dir("lmstudio-cfg");
    std::fs::create_dir_all(home.join(".lmstudio")).expect("root");
    let path = lmstudio_mcp_path_from(&home);

    let action = patch_cursor_like(&path, "mcpServers", "lmstudio", false, false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["command"], "nika");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));

    let again = patch_cursor_like(&path, "mcpServers", "lmstudio", false, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(home);
}

/// #449 wave-3b · cline: a host IDE's EXISTING extension dir wins (the
/// live hot-reloaded file); bare home falls back to the stable
/// `~/.cline/data/settings/` (the CLI's today + extension's next home).
#[test]
fn cline_path_resolution_truth_table() {
    let home = temp_dir("cline-home");

    // Bare home — the stable cross-generation path (created on write).
    assert_eq!(
        cline_settings_path_from(&home),
        home.join(".cline")
            .join("data")
            .join("settings")
            .join("cline_mcp_settings.json")
    );

    // A host IDE has the extension — its live globalStorage file wins.
    let host_root = if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("Cursor")
    } else {
        home.join(".config").join("Cursor")
    };
    let ext = host_root
        .join("User")
        .join("globalStorage")
        .join("saoudrizwan.claude-dev");
    std::fs::create_dir_all(&ext).expect("ext dir");
    assert_eq!(
        cline_settings_path_from(&home),
        ext.join("settings").join("cline_mcp_settings.json")
    );
    let _ = std::fs::remove_dir_all(home);
}

/// #449 wave-3b · cline: Cursor-style `mcpServers` record at the
/// resolved path — created with parents, then idempotent.
#[test]
fn cline_config_created_and_idempotent() {
    let home = temp_dir("cline-cfg");
    let path = cline_settings_path_from(&home);

    let action = patch_cursor_like(&path, "mcpServers", "cline", false, false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["command"], "nika");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));

    let again = patch_cursor_like(&path, "mcpServers", "cline", false, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(home);
}

/// #449 wave-3b · continue: the OWN-FILE drop-dir write
/// (`~/.continue/mcpServers/nika.json` · Claude-Desktop shape) — the
/// user's config.yaml is never touched; the reload hint rides Created/
/// Updated and stays OFF the idempotent re-run.
#[test]
fn continue_own_file_created_idempotent_and_hinted() {
    let home = temp_dir("continue");
    let path = home.join(".continue").join("mcpServers").join("nika.json");

    let action = patch_cursor_like(&path, "mcpServers", "continue", false, false)
        .map(|a| with_hint(a, "reload Continue"))
        .expect("wire");
    assert!(
        matches!(&action, WireAction::Created(label) if label.contains("reload Continue")),
        "created carries the hint: {action:?}"
    );
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["command"], "nika");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));

    let again = patch_cursor_like(&path, "mcpServers", "continue", false, false)
        .map(|a| with_hint(a, "reload Continue"))
        .expect("re-run");
    assert!(
        matches!(&again, WireAction::Current(label) if !label.contains("reload")),
        "current stays bare: {again:?}"
    );
    let _ = std::fs::remove_dir_all(home);
}

/// #449 · claude-desktop: the app-config dir differs per platform —
/// macOS `Library/Application Support/Claude/`, elsewhere
/// `.config/Claude/` — and is NEVER Claude Code's `~/.claude.json`.
#[test]
fn claude_desktop_path_is_the_app_config_dir() {
    let home = Path::new("/probe-home");
    let path = claude_desktop_config_path_from(home);
    let expected = if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("Claude")
    } else {
        home.join(".config").join("Claude")
    };
    assert_eq!(path, expected.join("claude_desktop_config.json"));
}

/// #449 · claude-desktop: `mcpServers` root (the modelcontextprotocol.io
/// quickstart shape) — created with parent dirs, then idempotent, other
/// servers preserved.
#[test]
fn claude_desktop_config_created_and_idempotent() {
    let home = temp_dir("claude-desktop");
    let path = claude_desktop_config_path_from(&home);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("app dir");
    std::fs::write(&path, r#"{"mcpServers":{"other":{"command":"x"}}}"#).expect("seed");

    let action =
        patch_cursor_like(&path, "mcpServers", "claude-desktop", false, false).expect("wire");
    assert!(matches!(action, WireAction::Updated(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["command"], "nika");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));
    assert_eq!(doc["mcpServers"]["other"]["command"], "x");

    let again =
        patch_cursor_like(&path, "mcpServers", "claude-desktop", false, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(home);
}

/// #449 · qwen: gemini-cli fork — same `mcpServers` key in a user-scope
/// `settings.json`, its own `.qwen` dotdir. Created then idempotent.
#[test]
fn qwen_settings_created_and_idempotent() {
    let home = temp_dir("qwen");
    let path = home.join(".qwen").join("settings.json");

    let action = patch_cursor_like(&path, "mcpServers", "qwen", false, false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["command"], "nika");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));

    let again = patch_cursor_like(&path, "mcpServers", "qwen", false, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(home);
}

/// #384 · junie: project-scope `.junie/mcp/mcp.json` — nested dirs
/// created, `mcpServers` root, idempotent.
#[test]
fn junie_project_config_created_nested_and_idempotent() {
    let project = temp_dir("junie");
    let path = project.join(".junie").join("mcp").join("mcp.json");

    let action = patch_cursor_like(&path, "mcpServers", "junie", false, false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)), "{action:?}");
    let doc = read_json(&path).expect("json");
    assert_eq!(doc["mcpServers"]["nika"]["command"], "nika");
    assert_eq!(doc["mcpServers"]["nika"]["args"], json!(["mcp"]));

    let again = patch_cursor_like(&path, "mcpServers", "junie", false, false).expect("re-run");
    assert!(matches!(again, WireAction::Current(_)));
    let _ = std::fs::remove_dir_all(project);
}

/// H7 · audit UX 2026-07-30: `wire all` rewrote every known client
/// config with no preview and no consent. The honest door is
/// preview (`--dry-run`) → detected (`wire detected`) → one client;
/// `all` asks on a terminal and refuses in a pipe without `--yes`.
#[test]
fn dry_run_lists_the_plan_and_writes_nothing() {
    let dir = temp_dir("dry-run");
    let dir_str = dir.to_str().expect("utf8 temp dir");
    let out = run_with(
        WireTarget::Vscode,
        dir_str,
        WireOptions {
            dry_run: true,
            ..WireOptions::default()
        },
    );
    assert_eq!(out.code, crate::output::exit::OK, "{out:?}");
    assert!(
        out.text.contains("dry-run"),
        "the plan says it is one: {out:?}"
    );
    assert!(
        out.text.contains("would create") && out.text.contains(".vscode"),
        "the plan names the action and the path: {out:?}"
    );
    assert!(
        !dir.join(".vscode").join("mcp.json").exists(),
        "a preview never writes"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// H7 · dry-run on an already-wired client reports `current` and
/// leaves the bytes untouched (no mtime-noise rewrite either).
#[test]
fn dry_run_reports_current_and_keeps_bytes_identical() {
    let dir = temp_dir("dry-run-current");
    let path = dir.join("opencode.json");
    let wired = r#"{"mcp":{"nika":{"type":"local","command":["nika","mcp"],"enabled":true}}}"#;
    std::fs::write(&path, wired).expect("seed");
    let out = run_with(
        WireTarget::Opencode,
        dir.to_str().expect("utf8"),
        WireOptions {
            dry_run: true,
            ..WireOptions::default()
        },
    );
    assert_eq!(out.code, crate::output::exit::OK, "{out:?}");
    assert!(out.text.contains("current"), "{out:?}");
    let after = std::fs::read_to_string(&path).expect("read");
    assert_eq!(after, wired, "byte-identical under a preview");
    let _ = std::fs::remove_dir_all(dir);
}

/// H7 · `detected`: only the clients THIS machine shows (the probe's
/// presence truth) — an absent client's config is never created.
#[test]
fn detected_targets_cover_only_present_clients() {
    let home = temp_dir("detected-home");
    let dir = temp_dir("detected-dir");
    assert!(
        detected_targets_in(&home, &dir).is_empty(),
        "a bare machine detects nothing"
    );

    std::fs::create_dir_all(home.join(".cursor")).expect("dir");
    std::fs::write(home.join(".cursor").join("mcp.json"), "{}").expect("seed");
    std::fs::write(home.join(".claude.json"), "{}").expect("seed");
    assert_eq!(
        detected_targets_in(&home, &dir),
        vec![WireTarget::Cursor, WireTarget::Claude],
        "probe order · present only"
    );

    std::fs::create_dir_all(dir.join(".vscode")).expect("dir");
    std::fs::write(dir.join(".vscode").join("mcp.json"), "{}").expect("seed");
    let found = detected_targets_in(&home, &dir);
    assert_eq!(found.len(), 3);
    assert!(found.contains(&WireTarget::Vscode), "workspace-scope seen");
    assert!(!found.contains(&WireTarget::Windsurf), "absent stays out");
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// A home holding the configs of five hosts: Codex (TOML, another
/// server, no `nika`), Gemini CLI (already wired), Copilot CLI, Amp and
/// Kimi Code (another server).
fn five_host_home(name: &str) -> PathBuf {
    let home = temp_dir(name);
    let seed = |parts: &[&str], body: &str| {
        let path = parts.iter().fold(home.clone(), |p, part| p.join(part));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, body).expect("seed");
    };
    seed(
        &[".codex", "config.toml"],
        "# my codex setup\n[mcp_servers.other]\ncommand = \"other\"\n",
    );
    seed(
        &[".gemini", "settings.json"],
        r#"{"mcpServers":{"nika":{"command":"nika","args":["mcp"]}}}"#,
    );
    seed(&[".copilot", "mcp-config.json"], r#"{"mcpServers":{}}"#);
    seed(&[".config", "amp", "settings.json"], "{}");
    seed(
        &[".kimi-code", "mcp.json"],
        r#"{"mcpServers":{"other":{"command":"x"}}}"#,
    );
    home
}

const FIVE_HOSTS: [WireTarget; 5] = [
    WireTarget::Codex,
    WireTarget::Gemini,
    WireTarget::Kimi,
    WireTarget::Copilot,
    WireTarget::Amp,
];

/// `detected` sees every host `nika wire` can write, not a list of its
/// own: a home holding five hosts lists exactly those, in row order, and
/// an empty home lists none. (Those five once answered « no MCP client
/// detected ».)
#[test]
fn detected_lists_exactly_the_hosts_this_home_holds() {
    let dir = temp_dir("five-hosts-dir");
    let bare = temp_dir("five-hosts-bare");
    assert_eq!(
        detected_targets_in(&bare, &dir),
        Vec::<WireTarget>::new(),
        "an empty home lists none"
    );

    let home = five_host_home("five-hosts-home");
    assert_eq!(detected_targets_in(&home, &dir), FIVE_HOSTS);
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(bare);
    let _ = std::fs::remove_dir_all(dir);
}

/// The preview of `detected` on that home: one line per host, each in
/// its own writer's words, and not a byte written.
#[test]
fn detected_dry_run_plans_each_host_it_sees_and_writes_nothing() {
    let home = five_host_home("five-hosts-plan-home");
    let dir = temp_dir("five-hosts-plan-dir");
    let at = |target| {
        config_path(target, Some(&home), &dir)
            .expect("a home-anchored path")
            .display()
            .to_string()
    };
    let bytes = || FIVE_HOSTS.map(|target| std::fs::read_to_string(at(target)).expect("read"));
    let before = bytes();

    let out = run_in(
        WireTarget::Detected,
        Some(&home),
        dir.to_str().expect("utf8"),
        WireOptions {
            dry_run: true,
            ..WireOptions::default()
        },
    );
    assert_eq!(out.code, crate::output::exit::OK, "{out:?}");
    assert_eq!(
        out.text,
        format!(
            "dry-run · plan only · nothing written\n\
             + would update codex: {}\n\
             · current gemini: {}\n\
             + would update kimi: {}\n\
             + would update copilot: {}\n\
             + would update amp: {}\n\
             → re-run without --dry-run to apply",
            at(WireTarget::Codex),
            at(WireTarget::Gemini),
            at(WireTarget::Kimi),
            at(WireTarget::Copilot),
            at(WireTarget::Amp),
        )
    );
    assert_eq!(bytes(), before, "a preview never writes");
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// A Codex config is TOML: with the `nika` server it reads current,
/// without it reads to-update, and reading it changes nothing.
#[test]
fn a_codex_toml_reads_current_with_the_nika_server_and_to_update_without() {
    let home = temp_dir("codex-detected-home");
    let dir = temp_dir("codex-detected-dir");
    let dir_str = dir.to_str().expect("utf8");
    let path = home.join(".codex").join("config.toml");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    let label = format!("codex: {}", path.display());

    let unwired = "# my codex setup\n[mcp_servers.github]\ncommand = \"gh\"\n";
    std::fs::write(&path, unwired).expect("seed");
    let targets = detected_targets_in(&home, &dir);
    assert_eq!(targets, [WireTarget::Codex]);
    assert_eq!(
        collect_plan(&targets, Some(&home), dir_str),
        [WireAction::Updated(label.clone())]
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        unwired,
        "a plan writes nothing"
    );

    let wired = format!("{unwired}\n[mcp_servers.nika]\ncommand = \"nika\"\nargs = [\"mcp\"]\n");
    std::fs::write(&path, &wired).expect("seed");
    assert_eq!(detected_targets_in(&home, &dir), [WireTarget::Codex]);
    assert_eq!(
        collect_plan(&targets, Some(&home), dir_str),
        [WireAction::Current(label)]
    );
    assert_eq!(std::fs::read_to_string(&path).expect("read"), wired);
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// The real run on that home wires the five hosts it sees, keeps every
/// other server they hold, and creates no config for a host that is not
/// there.
#[test]
fn detected_wires_the_hosts_it_sees_and_creates_nothing_for_the_others() {
    let home = five_host_home("five-hosts-run-home");
    let dir = temp_dir("five-hosts-run-dir");
    let dir_str = dir.to_str().expect("utf8");
    let at = |target| config_path(target, Some(&home), &dir).expect("a path");

    let out = run_in(
        WireTarget::Detected,
        Some(&home),
        dir_str,
        WireOptions::default(),
    );
    assert_eq!(out.code, crate::output::exit::OK, "{out:?}");
    assert_eq!(
        out.text,
        format!(
            "✔ updated codex: {}\n\
             · current gemini: {}\n\
             ✔ updated kimi: {}\n\
             ✔ updated copilot: {}\n\
             ✔ updated amp: {}",
            at(WireTarget::Codex).display(),
            at(WireTarget::Gemini).display(),
            at(WireTarget::Kimi).display(),
            at(WireTarget::Copilot).display(),
            at(WireTarget::Amp).display(),
        )
    );

    let codex: toml_edit::DocumentMut = std::fs::read_to_string(at(WireTarget::Codex))
        .expect("read")
        .parse()
        .expect("toml");
    assert_eq!(
        codex["mcp_servers"]["nika"]["command"].as_str(),
        Some("nika")
    );
    assert_eq!(
        codex["mcp_servers"]["other"]["command"].as_str(),
        Some("other"),
        "the other Codex server is kept"
    );
    let kimi = read_json(&at(WireTarget::Kimi)).expect("json");
    assert_eq!(kimi["mcpServers"]["nika"]["args"], json!(["mcp"]));
    assert_eq!(
        kimi["mcpServers"]["other"]["command"], "x",
        "the other Kimi server is kept"
    );
    let copilot = read_json(&at(WireTarget::Copilot)).expect("json");
    assert_eq!(copilot["mcpServers"]["nika"]["type"], "local");
    let amp = read_json(&at(WireTarget::Amp)).expect("json");
    assert_eq!(amp["amp.mcpServers"]["nika"]["command"], "nika");

    for target in expand_target(WireTarget::All) {
        if !FIVE_HOSTS.contains(&target) {
            assert!(
                !at(target).exists(),
                "{} is not on this machine and got a config",
                target.id()
            );
        }
    }
    assert_eq!(
        collect_plan(&FIVE_HOSTS, Some(&home), dir_str),
        FIVE_HOSTS.map(|target| WireAction::Current(format!(
            "{}: {}",
            target.id(),
            at(target).display()
        ))),
        "a second run has nothing to do"
    );
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// A client cannot be wired without being seen: every client `nika
/// wire` writes into a bare home is detected there afterwards, at the
/// file it was written to. A wire target added without a detection row
/// fails here.
#[test]
fn every_client_wired_into_a_bare_home_is_then_detected() {
    let home = temp_dir("round-trip-home");
    let dir = temp_dir("round-trip-dir");
    let dir_str = dir.to_str().expect("utf8");
    let all = expand_target(WireTarget::All);
    assert_eq!(detected_targets_in(&home, &dir), Vec::<WireTarget>::new());

    for target in &all {
        let action = wire_one(*target, Some(&home), dir_str, false).expect("wire");
        assert!(
            matches!(action, WireAction::Created(_)),
            "{}: {action:?}",
            target.id()
        );
    }
    let seen = detected_targets_in(&home, &dir);
    for target in &all {
        assert_eq!(
            seen.iter().filter(|one| *one == target).count(),
            1,
            "{} was wired and is not detected",
            target.id()
        );
    }
    assert_eq!(seen.len(), all.len(), "nothing else is detected: {seen:?}");
    for action in collect_plan(&seen, Some(&home), dir_str) {
        assert!(matches!(action, WireAction::Current(_)), "{action:?}");
    }
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// `detected` means the file the writer would patch is there. Cursor
/// also reads a project `.cursor/mcp.json` (`doctor` reports that
/// sighting), but a repository that ships one does not put Cursor on
/// this machine: nothing is planned for it and no home config is created.
#[test]
fn detected_plans_nothing_for_cursor_from_a_project_file_alone() {
    let home = temp_dir("project-cursor-home");
    let dir = temp_dir("project-cursor-dir");
    let project = dir.join(".cursor").join("mcp.json");
    std::fs::create_dir_all(project.parent().expect("parent")).expect("dir");
    let shipped = r#"{"mcpServers":{"other":{"command":"x"}}}"#;
    std::fs::write(&project, shipped).expect("seed");

    assert_eq!(detected_targets_in(&home, &dir), Vec::<WireTarget>::new());
    let out = run_in(
        WireTarget::Detected,
        Some(&home),
        dir.to_str().expect("utf8"),
        WireOptions::default(),
    );
    assert_eq!(out.code, crate::output::exit::OK, "{out:?}");
    assert!(out.text.starts_with("no MCP client detected"), "{out:?}");
    assert!(
        !home.join(".cursor").exists(),
        "no home config is created for a host that is not there"
    );
    assert_eq!(std::fs::read_to_string(&project).expect("read"), shipped);
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

const GEMINI_UNWIRED: &str = "{\"theme\":\"dark\"}\n";
const CODEX_REFUSED: &str = "mcp_servers = \"mine\"\n";

/// A home with two hosts: Gemini CLI, whose settings are healthy and not
/// wired yet, and Codex, whose config its writer refuses (`mcp_servers`
/// is not a table). Returns the home, then the Gemini and Codex files.
fn refused_codex_home(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let home = temp_dir(name);
    let gemini = home.join(".gemini").join("settings.json");
    let codex = home.join(".codex").join("config.toml");
    for (path, body) in [(&gemini, GEMINI_UNWIRED), (&codex, CODEX_REFUSED)] {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, body).expect("seed");
    }
    (home, gemini, codex)
}

/// Nobody named the hosts `detected` wires, the machine did: one config
/// its writer refuses is reported on its own line with the reason and
/// left byte-identical, the other hosts are wired, and the exit is ENV
/// so a script sees that one was skipped. The preview says the same.
#[test]
fn detected_wires_the_other_hosts_when_one_config_is_refused() {
    let (home, gemini, codex) = refused_codex_home("partial-home");
    let dir = temp_dir("partial-dir");
    let dir_str = dir.to_str().expect("utf8");
    let read = |path: &Path| std::fs::read_to_string(path).expect("read");
    let refusal = format!(
        "codex: `mcp_servers` in {} is not a TOML table; nika wire leaves it as it is",
        codex.display()
    );

    let plan = run_in(
        WireTarget::Detected,
        Some(&home),
        dir_str,
        WireOptions {
            dry_run: true,
            ..WireOptions::default()
        },
    );
    assert_eq!(plan.code, crate::output::exit::ENV, "{plan:?}");
    assert_eq!(
        plan.text,
        format!(
            "dry-run · plan only · nothing written\n\
             ✖ skipped {refusal}\n\
             + would update gemini: {}\n\
             → re-run without --dry-run to apply",
            gemini.display()
        )
    );
    assert_eq!(read(&gemini), GEMINI_UNWIRED, "a preview never writes");

    let out = run_in(
        WireTarget::Detected,
        Some(&home),
        dir_str,
        WireOptions::default(),
    );
    assert_eq!(out.code, crate::output::exit::ENV, "{out:?}");
    assert_eq!(
        out.text,
        format!(
            "✖ skipped {refusal}\n✔ updated gemini: {}",
            gemini.display()
        )
    );
    assert_eq!(
        read(&codex),
        CODEX_REFUSED,
        "the refused config is untouched"
    );
    let wired = read_json(&gemini).expect("json");
    assert_eq!(wired["mcpServers"]["nika"], nika_server(false));
    assert_eq!(wired["theme"], "dark", "the rest of the file is kept");
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// The same two hosts NAMED keep the all-or-nothing preflight: the
/// person chose those targets, so one refused config writes nothing.
#[test]
fn a_named_list_still_writes_nothing_when_one_config_is_refused() {
    let (home, gemini, codex) = refused_codex_home("named-home");
    let dir = temp_dir("named-dir");
    let out = wire_targets(
        &[WireTarget::Gemini, WireTarget::Codex],
        Some(&home),
        dir.to_str().expect("utf8"),
        false,
    );
    assert_eq!(out.code, crate::output::exit::ENV, "{out:?}");
    assert!(out.text.starts_with("nothing written"), "{out:?}");
    assert_eq!(
        std::fs::read_to_string(&gemini).expect("read"),
        GEMINI_UNWIRED
    );
    assert_eq!(
        std::fs::read_to_string(&codex).expect("read"),
        CODEX_REFUSED
    );
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// A `nika` entry that pins an absolute command and carries a key of its
/// own is wired: a desktop application does not inherit the shell's
/// PATH, and the bare name would break what works. `detected` plans no
/// change and leaves the file byte-identical; the same entry with other
/// args is to update. A client wired BY NAME keeps the strict writer.
#[test]
fn detected_leaves_a_pinned_entry_alone_and_updates_other_args() {
    let home = temp_dir("pinned-home");
    let dir = temp_dir("pinned-dir");
    let dir_str = dir.to_str().expect("utf8");
    let path = config_path(WireTarget::ClaudeDesktop, Some(&home), &dir).expect("path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    let label = format!("claude-desktop: {}", path.display());
    let entry = |args: &str| {
        format!(
            r#"{{"mcpServers":{{"nika":{{"command":"/opt/nika/bin/nika","args":{args},"env":{{"NIKA_HOME":"/data"}}}}}}}}"#
        )
    };
    let detected = |dry_run| {
        run_in(
            WireTarget::Detected,
            Some(&home),
            dir_str,
            WireOptions {
                dry_run,
                ..WireOptions::default()
            },
        )
    };

    let pinned = entry(r#"["mcp"]"#);
    std::fs::write(&path, &pinned).expect("seed");
    assert_eq!(
        detected(true).text,
        format!(
            "dry-run · plan only · nothing written\n\
             · current {label}\n\
             → re-run without --dry-run to apply"
        )
    );
    let out = detected(false);
    assert_eq!(out.code, crate::output::exit::OK, "{out:?}");
    assert_eq!(out.text, format!("· current {label}"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        pinned,
        "a wired entry is left byte-identical"
    );
    assert_eq!(
        wire_one(WireTarget::ClaudeDesktop, Some(&home), dir_str, true),
        Ok(WireAction::Updated(label.clone())),
        "wired by name, the writer stays strict"
    );

    std::fs::write(&path, entry(r#"["mcp","--verbose"]"#)).expect("seed");
    assert_eq!(
        detected(true).text,
        format!(
            "dry-run · plan only · nothing written\n\
             + would update {label}\n\
             → re-run without --dry-run to apply"
        )
    );
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// One rule for every host, the six standing rows included: `doctor`
/// has always called a pinned Cursor entry wired, and `detected` no
/// longer rewrites what `doctor` calls wired. The plan is no change and
/// the file stays byte-identical. Cursor NAMED by hand keeps the strict
/// writer: the entry is rewritten into nika's own spelling.
#[test]
fn detected_leaves_a_pinned_cursor_entry_alone_and_a_named_wire_rewrites_it() {
    let home = temp_dir("pinned-cursor-home");
    let dir = temp_dir("pinned-cursor-dir");
    let dir_str = dir.to_str().expect("utf8");
    let path = home.join(".cursor").join("mcp.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    let label = format!("cursor: {}", path.display());
    let pinned = r#"{"mcpServers":{"nika":{"command":"/opt/nika/bin/nika","args":["mcp"],"env":{"NIKA_HOME":"/data"}}}}"#;
    std::fs::write(&path, pinned).expect("seed");
    let wire = |target, dry_run| {
        run_in(
            target,
            Some(&home),
            dir_str,
            WireOptions {
                dry_run,
                ..WireOptions::default()
            },
        )
    };

    assert_eq!(
        wire(WireTarget::Detected, true).text,
        format!(
            "dry-run · plan only · nothing written\n\
             · current {label}\n\
             → re-run without --dry-run to apply"
        )
    );
    let out = wire(WireTarget::Detected, false);
    assert_eq!(out.code, crate::output::exit::OK, "{out:?}");
    assert_eq!(out.text, format!("· current {label}"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        pinned,
        "a wired entry is left byte-identical"
    );

    let named = wire(WireTarget::Cursor, false);
    assert_eq!(named.code, crate::output::exit::OK, "{named:?}");
    assert_eq!(named.text, format!("✔ updated {label}"));
    assert_eq!(
        read_json(&path).expect("json")["mcpServers"]["nika"],
        nika_server(false),
        "named by hand, the entry is rewritten as the writer spells it"
    );
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// The reader cannot drift from the writers: on every host whose config
/// is JSON or TOML (all but Hermes, whose YAML is never rewritten), what
/// the writer wrote still reads wired once its command is pinned to an
/// absolute path, in that host's own table and shape.
#[test]
fn a_pinned_command_reads_wired_on_every_json_and_toml_host() {
    let home = temp_dir("pinned-all-home");
    let dir = temp_dir("pinned-all-dir");
    let hosts: Vec<WireTarget> = crate::detect::sights(Some(&home), &dir)
        .iter()
        .map(|sight| sight.target)
        .filter(|target| *target != WireTarget::Hermes)
        .collect();
    assert_eq!(hosts.len(), 20, "every wire target but Hermes");

    for target in hosts {
        let path = config_path(target, Some(&home), &dir).expect("path");
        wire_at(target, &path, false).expect("wire");
        let written = std::fs::read_to_string(&path).expect("read");
        assert_eq!(reads(target, &path), (true, false), "{}", target.id());
        let pinned = written
            .replace(
                "\"command\": \"nika\"",
                "\"command\": \"/opt/nika/bin/nika\"",
            )
            .replace("\"nika\",", "\"/opt/nika/bin/nika\",")
            .replace("command = \"nika\"", "command = \"/opt/nika/bin/nika\"");
        assert_ne!(pinned, written, "{}: no command was pinned", target.id());
        std::fs::write(&path, &pinned).expect("pin");
        assert_eq!(reads(target, &path), (true, false), "{}", target.id());
        assert!(
            matches!(wire_at(target, &path, true), Ok(WireAction::Updated(_))),
            "{}: wired by name, the writer stays strict",
            target.id()
        );
    }
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

/// A settings path that is not a settings file (a directory, a FIFO, a
/// link to `/dev/zero`, a file past the bound, one this account cannot
/// read, an empty one) is refused with its reason, promptly, by the
/// `detected` preview and by the run: its own skipped line, exit ENV.
fn detected_refuses(hazard: crate::detect::hazards::Hazard) {
    let home = temp_dir(&format!("hazard-{hazard:?}-home"));
    let dir = temp_dir(&format!("hazard-{hazard:?}-dir"));
    let path = home.join(".gemini").join("settings.json");
    let Some(reason) = hazard.plant(&path) else {
        return; // root reads through a 000 mode: nothing to stage
    };
    for dry_run in [true, false] {
        let (home, dir) = (home.clone(), dir.clone());
        let out = crate::detect::hazards::promptly(move || {
            run_in(
                WireTarget::Detected,
                Some(&home),
                dir.to_str().expect("utf8"),
                WireOptions {
                    dry_run,
                    ..WireOptions::default()
                },
            )
        });
        assert_eq!(out.code, crate::output::exit::ENV, "{hazard:?}: {out:?}");
        let line = out
            .text
            .lines()
            .find(|line| line.starts_with("✖ skipped"))
            .unwrap_or_default();
        assert!(
            line.contains(&path.display().to_string()) && line.contains(reason),
            "{hazard:?} (dry-run {dry_run}): {out:?}"
        );
    }
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn detected_refuses_a_directory() {
    detected_refuses(crate::detect::hazards::Hazard::Directory);
}

#[test]
fn detected_refuses_an_empty_file() {
    detected_refuses(crate::detect::hazards::Hazard::Empty);
}

#[test]
fn detected_refuses_an_unreadable_file() {
    detected_refuses(crate::detect::hazards::Hazard::Unreadable);
}

#[test]
fn detected_refuses_a_link_to_dev_zero() {
    detected_refuses(crate::detect::hazards::Hazard::DevZeroLink);
}

#[test]
fn detected_refuses_a_fifo() {
    detected_refuses(crate::detect::hazards::Hazard::Fifo);
}

#[test]
fn detected_refuses_an_oversized_file() {
    detected_refuses(crate::detect::hazards::Hazard::Oversized);
}

/// H7 · the consent gate is PURE (the `model pull` confirm-gate
/// precedent): dry-run or `--yes` proceeds, a terminal prompts, a
/// bare pipe is refused.
#[test]
fn all_consent_truth_table() {
    assert_eq!(all_consent(true, false, false), AllConsent::Proceed);
    assert_eq!(all_consent(false, true, false), AllConsent::Proceed);
    assert_eq!(all_consent(false, false, true), AllConsent::Prompt);
    assert_eq!(all_consent(false, false, false), AllConsent::Refuse);
}

/// H7 · a bare `wire all` in a pipe refuses HONESTLY: exit 3, and
/// the refusal names the right door (preview · detected · --yes).
#[test]
fn wire_all_without_consent_is_refused_naming_the_path() {
    let out = run_with(WireTarget::All, ".", WireOptions::default());
    assert_eq!(out.code, crate::output::exit::ENV, "{out:?}");
    assert!(out.text.contains("--dry-run"), "{out:?}");
    assert!(out.text.contains("wire detected"), "{out:?}");
    assert!(out.text.contains("--yes"), "{out:?}");
}

/// H7 · `wire all --dry-run` is the preview — no consent needed,
/// nothing written (the temp workspace stays empty). The exit code
/// PREDICTS the run's: a client whose existing config is unreadable
/// is a `✖ skipped` line and flips the code to ENV — on a machine
/// with all-healthy configs the preview exits OK.
#[test]
fn wire_all_dry_run_previews_without_consent() {
    let dir = temp_dir("all-dry-run");
    let out = run_with(
        WireTarget::All,
        dir.to_str().expect("utf8"),
        WireOptions {
            dry_run: true,
            ..WireOptions::default()
        },
    );
    assert!(
        out.code == crate::output::exit::OK
            || (out.code == crate::output::exit::ENV && out.text.contains("skipped")),
        "the exit code predicts the run's (skips included): {out:?}"
    );
    assert!(out.text.contains("dry-run"), "{out:?}");
    assert!(
        out.text.contains("opencode.json"),
        "the workspace-scope client is planned: {out:?}"
    );
    assert!(
        !dir.join("opencode.json").exists(),
        "a preview never writes"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[allow(clippy::disallowed_methods)]
fn temp_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR").map_or_else(
        || {
            std::env::current_dir()
                .expect("current dir")
                .join("target")
                .join("tmp")
        },
        PathBuf::from,
    );
    let dir = base.join(format!("nika-wire-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The atomic write (audit 2026-07-31): every config write lands via a
/// same-dir temp file + fsync + rename — a crash mid-write can no
/// longer destroy the user's whole config. A NEW file is 0600 (these
/// configs grow API keys); an existing file keeps its mode through the
/// rename; no temp litter remains.
#[test]
fn atomic_write_creates_0600_preserves_mode_and_leaves_no_litter() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = temp_dir("atomic");
    let path = dir.join("mcp.json");

    // Create → 0600, exact bytes.
    write_atomic(&path, "{\"a\":1}\n").expect("write");
    let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "a new config is owner-only, got {mode:o}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "{\"a\":1}\n",
        "the content is the post-rename body"
    );

    // Overwrite with a custom mode → the mode survives the rename.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
    write_atomic(&path, "{\"a\":2}\n").expect("rewrite");
    let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o640, "an existing mode is preserved, got {mode:o}");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"a\":2}\n");

    // The temp file is renamed away — no `.nika-tmp-*` litter.
    let litter: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir")
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(".nika-tmp-"))
        .collect();
    assert!(litter.is_empty(), "no temp litter: {litter:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Both wire writers route through the atomic house: a fresh TOML/JSON
/// config lands 0600, and a pre-existing file's mode survives the
/// update (the truncate-in-place era rewrote it at the umask).
#[test]
fn writers_route_through_the_atomic_house() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = temp_dir("atomic-routes");
    let mode_of = |path: &Path| std::fs::metadata(path).expect("meta").permissions().mode() & 0o777;

    // The TOML writer, create path.
    let toml = dir.join("config.toml");
    let action = patch_toml_mcp(&toml, "codex", false).expect("wire");
    assert!(matches!(action, WireAction::Created(_)), "{action:?}");
    assert_eq!(mode_of(&toml), 0o600, "a new TOML config is owner-only");

    // The JSON writer, create path.
    let json = dir.join("mcp.json");
    write_json(&json, &json!({"mcpServers": {}}), None).expect("json");
    assert_eq!(mode_of(&json), 0o600, "a new JSON config is owner-only");

    // The JSON writer over a pre-existing 0640 file preserves the mode.
    std::fs::set_permissions(&json, std::fs::Permissions::from_mode(0o640)).expect("chmod");
    let before = std::fs::read_to_string(&json).expect("before");
    write_json(
        &json,
        &json!({"mcpServers": {"nika": {"command": "nika"}}}),
        Some(&before),
    )
    .expect("rewrite");
    assert_eq!(mode_of(&json), 0o640, "an existing mode is preserved");
    let _ = std::fs::remove_dir_all(dir);
}

/// A server table that is not a JSON object is the user's: every
/// JSON writer refuses it, names why, and leaves the file byte-identical
/// (it once became `{}`, the user's servers erased).
#[test]
fn wire_refuses_a_non_object_server_table_without_writing() {
    type Patch = fn(&Path, bool) -> Result<WireAction, String>;
    let dir = temp_dir("non-object-table");
    let families: [(&str, &str, Patch); 6] = [
        ("vscode.json", "servers", patch_vscode),
        ("cursor.json", "mcpServers", |path, dry| {
            patch_cursor_like(path, "mcpServers", "cursor", false, dry)
        }),
        ("opencode.json", "mcp", patch_opencode),
        ("copilot.json", "mcpServers", patch_copilot),
        ("zed.json", "context_servers", patch_zed),
        ("amp.json", "amp.mcpServers", patch_amp),
    ];
    for (file, key, patch) in families {
        let path = dir.join(file);
        let seeded = format!("{{\"{key}\":[\"keep\"],\"x\":1}}");
        std::fs::write(&path, &seeded).expect("seed");
        for dry_run in [true, false] {
            let refused = patch(&path, dry_run).expect_err("a non-object table is refused");
            assert!(
                refused.contains("is not a JSON object"),
                "{file}: {refused}"
            );
            assert_eq!(
                std::fs::read_to_string(&path).expect("read"),
                seeded,
                "{file}: byte-identical"
            );
        }
    }
    let toml = dir.join("config.toml");
    std::fs::write(&toml, "mcp_servers = [\"keep\"]\n").expect("seed toml");
    let refused = patch_toml_mcp(&toml, "codex", false).expect_err("refused");
    assert!(refused.contains("is not a TOML table"), "{refused}");
    assert_eq!(
        std::fs::read_to_string(&toml).expect("read"),
        "mcp_servers = [\"keep\"]\n"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The preview of such a config says `skipped`, never « would
/// update », and exits ENV.
#[test]
fn dry_run_of_a_non_object_table_says_skipped() {
    let dir = temp_dir("non-object-dry-run");
    std::fs::create_dir_all(dir.join(".vscode")).expect("dir");
    let seeded = r#"{"servers":["keep"],"x":1}"#;
    std::fs::write(dir.join(".vscode").join("mcp.json"), seeded).expect("seed");
    let out = run_with(
        WireTarget::Vscode,
        dir.to_str().expect("utf8"),
        WireOptions {
            dry_run: true,
            ..WireOptions::default()
        },
    );
    assert_eq!(out.code, crate::output::exit::ENV, "{out:?}");
    assert!(
        out.text.contains("✖ skipped") && !out.text.contains("would update"),
        "{out:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(".vscode").join("mcp.json")).expect("read"),
        seeded
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A named client and `all` preflight every client before the first
/// write: one malformed config leaves every config untouched, exit ENV,
/// never the first client written before the second is found invalid.
/// (`detected` wires host by host instead, see
/// `detected_wires_the_other_hosts_when_one_config_is_refused`. Driven
/// through the list door with project-scope clients, so no test ever
/// reads this machine's HOME.)
#[test]
fn a_named_list_writes_nothing_when_one_client_is_malformed() {
    let dir = temp_dir("detected-preflight");
    let opencode = r#"{"mcp":["keep"]}"#;
    std::fs::write(dir.join("opencode.json"), opencode).expect("seed");
    let out = wire_targets(
        &[WireTarget::Vscode, WireTarget::Opencode, WireTarget::Junie],
        None,
        dir.to_str().expect("utf8"),
        false,
    );
    assert_eq!(out.code, crate::output::exit::ENV, "{out:?}");
    assert!(out.text.starts_with("nothing written"), "{out:?}");
    assert!(
        !dir.join(".vscode").join("mcp.json").exists(),
        "the first client is never written before the second is found invalid"
    );
    assert!(!dir.join(".junie").exists(), "nor the third");
    assert_eq!(
        std::fs::read_to_string(dir.join("opencode.json")).expect("read"),
        opencode
    );
    // Once the config is repaired, the same run wires all three.
    std::fs::write(dir.join("opencode.json"), "{}").expect("repair");
    let out = wire_targets(
        &[WireTarget::Vscode, WireTarget::Opencode, WireTarget::Junie],
        None,
        dir.to_str().expect("utf8"),
        false,
    );
    assert_eq!(out.code, crate::output::exit::OK, "{out:?}");
    assert!(dir.join(".vscode").join("mcp.json").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// Every write is read back: when the file does not hold what was
/// written, it is restored as it was (its original text, or no file when
/// there was none) and the write fails, naming why.
#[test]
fn a_write_that_does_not_read_back_is_restored() {
    let dir = temp_dir("readback");
    let path = dir.join("mcp.json");
    let tampering: fn(&Path, &str) -> Result<(), String> =
        |path, _| write_atomic(path, "{\"tampered\":true}\n");

    std::fs::write(&path, "{\"x\":1}\n").expect("seed");
    let failed = write_read_back(&path, "{\"nika\":1}\n", Some("{\"x\":1}\n"), tampering)
        .expect_err("a mismatch fails");
    assert!(failed.contains("restored as it was"), "{failed}");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"x\":1}\n");

    let fresh = dir.join("fresh.json");
    let failed =
        write_read_back(&fresh, "{\"nika\":1}\n", None, tampering).expect_err("a mismatch fails");
    assert!(failed.contains("restored as it was"), "{failed}");
    assert!(
        !fresh.exists(),
        "a file this write created is removed again"
    );

    write_read_back(&path, "{\"nika\":2}\n", Some("{\"x\":1}\n"), write_atomic)
        .expect("a faithful write reads back");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "{\"nika\":2}\n"
    );
    let _ = std::fs::remove_dir_all(dir);
}
