// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! Execute the generated authoring setup with the real binary and an empty HOME.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::panic, clippy::disallowed_types)]

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn isolated(program: &Path, home: &Path, cwd: &Path) -> Command {
    let binary_dir = Path::new(env!("CARGO_BIN_EXE_nika"))
        .parent()
        .expect("binary directory");
    let path = std::env::join_paths([binary_dir, Path::new("/usr/bin"), Path::new("/bin")])
        .expect("executable search path");
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("NIKA_KEYCHAIN", "off")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .current_dir(cwd)
        .stdin(Stdio::null());
    command
}

fn nika(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    isolated(Path::new(env!("CARGO_BIN_EXE_nika")), home, cwd)
        .args(args)
        .output()
        .expect("nika executes")
}

fn transcript(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn init_preserves_detected_scope_and_reports_an_all_refusal() {
    let home = tempfile::tempdir().expect("isolated home");
    let project = home.path().join("project");
    std::fs::create_dir(&project).expect("project");
    let detected = nika(
        home.path(),
        &project,
        &["init", "--yes", "--wire", "detected"],
    );
    assert_eq!(detected.status.code(), Some(0), "{}", transcript(&detected));
    assert!(!transcript(&detected).contains("wire all rewrites"));
    assert!(
        !home.path().join(".codex").exists(),
        "no undetected home configuration"
    );

    let refused = nika(home.path(), &project, &["init", "--yes", "--wire", "all"]);
    assert_eq!(refused.status.code(), Some(3), "{}", transcript(&refused));
    assert!(transcript(&refused).contains("wire all --yes"));
    assert!(!transcript(&refused).contains("wired ·"));
    assert!(
        project.join("AGENTS.md").is_file(),
        "completed scaffold is retained"
    );
    assert!(!home.path().join(".codex").exists());
}

fn hook(
    home: &Path,
    project: &Path,
    cwd: &Path,
    command: &str,
    payload: &serde_json::Value,
) -> Output {
    let mut child = isolated(Path::new("/bin/sh"), home, cwd)
        .args(["-c", command])
        .env("CLAUDE_PROJECT_DIR", project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("generated command starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("payload");
    child.wait_with_output().expect("hook completes")
}

#[test]
fn claude_hooks_execute_from_a_nested_directory_and_preserve_user_settings() {
    let home = tempfile::tempdir().expect("isolated home");
    let project = home.path().join("project with spaces");
    let nested = project.join("nested");
    std::fs::create_dir_all(&nested).expect("project");
    let init = nika(home.path(), &project, &["init", "--yes"]);
    assert!(init.status.success(), "{}", transcript(&init));
    let settings_path = project.join(".claude/settings.json");
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&settings_path).expect("settings"))
            .expect("valid settings");
    let command = |event: &str| {
        settings["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .expect("generated command")
    };
    let session = hook(
        home.path(),
        &project,
        &nested,
        command("SessionStart"),
        &serde_json::json!({"hook_event_name": "SessionStart", "cwd": project}),
    );
    assert!(session.status.success(), "{}", transcript(&session));
    let context: serde_json::Value =
        serde_json::from_slice(&session.stdout).expect("Claude envelope");
    assert_eq!(
        context["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    assert!(
        context["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("context")
            .contains("nika")
    );

    let workflow = project.join("dirty.nika");
    std::fs::write(&workflow, "nika: dirty\npermits: {}\ntasks:\n  read:\n    invoke:\n      tool: nika:read\n      args: {path: secret.txt}\n").expect("negative workflow");
    let edit = hook(
        home.path(),
        &project,
        &nested,
        command("PostToolUse"),
        &serde_json::json!({"hook_event_name": "PostToolUse", "tool_input": {"file_path": workflow}, "cwd": project}),
    );
    assert_eq!(edit.status.code(), Some(2), "{}", transcript(&edit));
    assert!(transcript(&edit).contains("NIKA-SEC-"));
    let guard = hook(
        home.path(),
        &project,
        &nested,
        command("PreToolUse"),
        &serde_json::json!({"hook_event_name": "PreToolUse", "tool_input": {"command": "nika run dirty.nika"}, "cwd": project}),
    );
    // A deny is the hook's own exit 2, the refusal every host reads alike:
    // the verdict rides stderr, ahead of the fixed guard_denied line, so the
    // findings still reach the agent.
    assert_eq!(
        guard.status.code(),
        Some(2),
        "the hook blocks the deny: {}",
        transcript(&guard)
    );
    assert!(guard.stdout.is_empty(), "{}", transcript(&guard));
    let refusal = String::from_utf8(guard.stderr.clone()).expect("utf8 stderr");
    let (verdict, fixed) = refusal
        .trim_end()
        .rsplit_once('\n')
        .expect("the verdict, then the fixed line");
    assert_eq!(
        fixed,
        "guard_denied: nika guard denied this nika run; the verdict above says why. Repair what it names, then retry."
    );
    let verdict: serde_json::Value = serde_json::from_str(verdict).expect("guard envelope");
    assert_eq!(verdict["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        verdict["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason")
            .contains("NIKA-SEC-004"),
        "{refusal}"
    );

    let own_settings = b"{\"env\":{\"TEAM_SETTING\":\"preserved\"}}\n";
    std::fs::write(&settings_path, own_settings).expect("user configuration");
    let again = nika(home.path(), &project, &["init", "--yes"]);
    assert!(again.status.success(), "{}", transcript(&again));
    assert_eq!(
        std::fs::read(&settings_path).expect("settings retained"),
        own_settings
    );
}

/// An executable `nika-kit` the repository ships where the plugin keeps its
/// resolver (`.claude/bin/`, `.cursor/bin/`, beside each `hooks-nika/`).
fn plant_resolver(path: &Path, marker: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(path.parent().expect("bin folder")).expect("mkdir");
    std::fs::write(
        path,
        format!("#!/bin/sh\necho \"$@\" >>'{}'\n", marker.display()),
    )
    .expect("planted resolver");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("exec bit");
}

/// The scaffolded edit hook judges with the binary the order picks (here
/// the PATH's, the binary under test) and never runs a resolver the
/// repository holds beside it, in either client's copy.
#[test]
fn a_resolver_the_repository_ships_beside_the_hooks_never_runs() {
    let home = tempfile::tempdir().expect("isolated home");
    let project = home.path().join("project");
    std::fs::create_dir(&project).expect("project");
    let marker = home.path().join("planted-resolver-ran");
    for client in [".claude", ".cursor"] {
        plant_resolver(&project.join(client).join("bin/nika-kit"), &marker);
    }
    let init = nika(home.path(), &project, &["init", "--yes"]);
    assert_eq!(init.status.code(), Some(0), "{}", transcript(&init));
    let workflow = project.join("workflows/01-hello.nika");
    assert!(workflow.is_file(), "init founds the hello lesson");
    let settings: serde_json::Value = serde_json::from_slice(
        &std::fs::read(project.join(".claude/settings.json")).expect("settings"),
    )
    .expect("valid settings");
    let claude = settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .expect("generated edit command")
        .to_owned();
    let cursor: serde_json::Value = serde_json::from_slice(
        &std::fs::read(project.join(".cursor/hooks.json")).expect("cursor hooks"),
    )
    .expect("valid cursor hooks");
    let cursor = cursor["hooks"]["afterFileEdit"][0]["command"]
        .as_str()
        .expect("generated cursor edit command")
        .to_owned();
    let judge = format!("clean · {}", env!("CARGO_BIN_EXE_nika"));
    for (command, payload) in [
        (
            format!("NIKA_CHECK_ANNOUNCE_CLEAN=1 {claude}"),
            serde_json::json!({"hook_event_name": "PostToolUse", "tool_input": {"file_path": workflow}, "cwd": project}),
        ),
        (
            format!("NIKA_CHECK_ANNOUNCE_CLEAN=1 {cursor}"),
            serde_json::json!({"file_path": workflow}),
        ),
    ] {
        let edit = hook(home.path(), &project, &project, &command, &payload);
        let text = transcript(&edit);
        assert_eq!(edit.status.code(), Some(0), "{command}: {text}");
        assert!(
            text.contains(&judge),
            "{command} judges with the PATH's nika: {text}"
        );
    }
    assert!(
        !marker.exists(),
        "a scaffolded hook ran the repository's resolver: {}",
        std::fs::read_to_string(&marker).unwrap_or_default()
    );
}

/// `init` never wires a client to a hook script it did not write: a script
/// already there with other bytes keeps `.claude/settings.json` unwritten,
/// with one report row naming both files and exit 3. A script holding
/// exactly what init writes is wired, and `--force` rewrites then wires.
#[test]
fn init_refuses_to_wire_a_hook_script_it_did_not_write() {
    let home = tempfile::tempdir().expect("isolated home");
    let project = home.path().join("project");
    let script = project.join(".claude/hooks-nika/check-on-edit.sh");
    std::fs::create_dir_all(script.parent().expect("hooks folder")).expect("project");
    let planted = format!(
        "#!/bin/sh\necho planted >>'{}'\n",
        home.path().join("planted-hook-ran").display()
    );
    std::fs::write(&script, &planted).expect("planted script");

    let refused = nika(home.path(), &project, &["init", "--yes"]);
    let text = transcript(&refused);
    assert_eq!(refused.status.code(), Some(3), "{text}");
    assert!(
        text.lines().any(|line| line
            == "✖ refused ./.claude/settings.json — its hooks would run ./.claude/hooks-nika/check-on-edit.sh, which init did not write · remove it and run nika init again, or rerun with --force, which rewrites every file init writes, AGENTS.md, CLAUDE.md, .mcp.json and settings included"),
        "{text}"
    );
    assert!(
        text.lines().any(|line| line
            == "· skipped ./.claude/hooks-nika/check-on-edit.sh (exists · --force to overwrite)"),
        "{text}"
    );
    assert!(!project.join(".claude/settings.json").exists(), "{text}");
    assert_eq!(
        std::fs::read_to_string(&script).expect("script kept"),
        planted
    );
    assert!(project.join(".cursor/hooks.json").is_file(), "{text}");
    assert!(project.join("AGENTS.md").is_file(), "{text}");
    assert!(!home.path().join("planted-hook-ran").exists());

    // The bytes init writes, taken from a fresh scaffold, are init's own.
    let fresh = home.path().join("fresh");
    std::fs::create_dir(&fresh).expect("fresh project");
    let founded = nika(home.path(), &fresh, &["init", "--yes"]);
    assert_eq!(founded.status.code(), Some(0), "{}", transcript(&founded));
    let own = std::fs::read(fresh.join(".claude/hooks-nika/check-on-edit.sh")).expect("own");
    let twin = home.path().join("twin");
    std::fs::create_dir_all(twin.join(".claude/hooks-nika")).expect("twin project");
    std::fs::write(twin.join(".claude/hooks-nika/check-on-edit.sh"), &own).expect("same bytes");
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            twin.join(".claude/hooks-nika/check-on-edit.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("the mode init gives it");
    }
    let accepted = nika(home.path(), &twin, &["init", "--yes"]);
    assert_eq!(accepted.status.code(), Some(0), "{}", transcript(&accepted));
    assert!(twin.join(".claude/settings.json").is_file());

    let forced = nika(home.path(), &project, &["init", "--yes", "--force"]);
    assert_eq!(forced.status.code(), Some(0), "{}", transcript(&forced));
    assert_eq!(std::fs::read(&script).expect("rewritten"), own);
    assert!(project.join(".claude/settings.json").is_file());
    assert!(!home.path().join("planted-hook-ran").exists());
}

#[test]
fn human_guide_first_workflow_runs_offline_and_project_receipt_precedes_next() {
    let home = tempfile::tempdir().expect("isolated home");
    let project = home.path().join("project");
    std::fs::create_dir(&project).expect("project");
    std::fs::write(project.join("README.md"), "existing project readme\n").expect("readme");
    let init = nika(home.path(), &project, &["init", "--yes", "--project-file"]);
    assert!(init.status.success(), "{}", transcript(&init));
    let text = transcript(&init);
    // #1283 · the project file is laid by default: its receipt rides the
    // report with its purpose, before the `next ·` hand-off.
    let receipt = text
        .lines()
        .position(|line| line.starts_with("✔ created ") && line.contains("nika.yaml — "))
        .expect("the project-file receipt rides the report");
    let next = text
        .lines()
        .position(|line| line.starts_with("next ·"))
        .expect("the hand-off block");
    assert!(receipt < next, "{text}");
    assert_eq!(
        std::fs::read_to_string(project.join("README.md")).expect("readme retained"),
        "existing project readme\n"
    );
    let guide = std::fs::read_to_string(project.join("NIKA.md")).expect("human guide");
    let commands = guide
        .split("```sh\n")
        .nth(1)
        .expect("first commands")
        .split("```")
        .next()
        .expect("commands end");
    for command in commands.lines() {
        let args: Vec<_> = command.split_whitespace().skip(1).collect();
        let result = nika(home.path(), &project, &args);
        assert!(
            result.status.success(),
            "guide command {command}: {}",
            transcript(&result)
        );
    }
}

/// A founded repository keeps the `nika serve` credential and job state out
/// of Git: the token where every serve surface teaches to mint it
/// (`.nika/serve.token`) and the default state root (`.nika/serve/`), beside
/// the traces. Git itself judges the rules, not a search of `.gitignore`.
#[test]
fn a_founded_repository_ignores_the_serve_token_and_job_state() {
    let home = tempfile::tempdir().expect("isolated home");
    let project = home.path().join("project");
    std::fs::create_dir(&project).expect("project");
    let init = nika(home.path(), &project, &["init", "--yes"]);
    assert!(init.status.success(), "{}", transcript(&init));
    let git = |args: &[&str]| {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(&project)
            .env("HOME", home.path());
        // An inherited GIT_DIR/GIT_WORK_TREE would aim at the caller's checkout.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(&key);
            }
        }
        command
            .output()
            .expect("git is on PATH (CI runners ship it)")
    };
    let created = git(&["init", "-q"]);
    assert!(created.status.success(), "{}", transcript(&created));
    for path in [
        ".nika/serve.token",
        ".nika/serve/jobs/state.json",
        ".nika/traces/run.ndjson",
    ] {
        let verdict = git(&["check-ignore", "-q", path]);
        assert_eq!(
            verdict.status.code(),
            Some(0),
            "{path} must be ignored in a founded repository: {}",
            transcript(&verdict)
        );
    }
}

/// Every file outside `project` under `top`, with its bytes or link target.
fn outside_of(top: &Path, project: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut seen = Vec::new();
    let mut todo = vec![top.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("list")
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.starts_with(project) {
                continue;
            }
            let kind = std::fs::symlink_metadata(&path).expect("stat");
            if kind.file_type().is_symlink() {
                let to = std::fs::read_link(&path).expect("link");
                seen.push((path, format!("-> {}", to.display())));
            } else if kind.is_dir() {
                todo.push(path);
            } else {
                let bytes = std::fs::read(&path).expect("read");
                seen.push((path, String::from_utf8_lossy(&bytes).into_owned()));
            }
        }
    }
    seen.sort();
    seen
}

/// The real binary, with HOME as the fake user: a project link aimed at the
/// user's global Claude settings, dangling or present, and a `.claude`
/// folder that is itself a link, never carry a write outside the project,
/// `--force` included.
#[test]
fn init_never_writes_through_a_link_to_the_user_settings() {
    let home = tempfile::tempdir().expect("isolated home");
    let top = home.path().canonicalize().expect("canonical home");
    let global = top.join(".claude");
    std::fs::create_dir_all(&global).expect("global settings folder");

    // A dangling link, then the same link with the global file present.
    let project = top.join("src/linked-settings");
    std::fs::create_dir_all(project.join(".claude")).expect("project");
    std::os::unix::fs::symlink(
        "../../../.claude/settings.json",
        project.join(".claude/settings.json"),
    )
    .expect("settings link");
    for (present, args) in [
        (false, &["init", "--yes"][..]),
        (false, &["init", "--yes", "--force"][..]),
        (true, &["init", "--yes"][..]),
        (true, &["init", "--yes", "--force"][..]),
    ] {
        if present {
            std::fs::write(global.join("settings.json"), "{\"the\": \"user's\"}\n")
                .expect("global settings");
        }
        let before = outside_of(&top, &project);
        let out = nika(&top, &project, args);
        let text = transcript(&out);
        // Without --force init would only skip the file, so the link is kept
        // and never followed; --force needs a write, which is refused.
        let (code, row) = if args.contains(&"--force") {
            (
                3,
                "✖ ./.claude/settings.json: a link — init never writes through a link, --force included · remove the link, then run nika init again",
            )
        } else {
            (
                0,
                "· skipped ./.claude/settings.json (a link leading out of the project or to nothing · kept, never followed)",
            )
        };
        assert_eq!(out.status.code(), Some(code), "{args:?}\n{text}");
        assert!(text.lines().any(|line| line == row), "{args:?}\n{text}");
        assert_eq!(
            outside_of(&top, &project),
            before,
            "{args:?}: nothing outside changed"
        );
        assert_eq!(global.join("settings.json").exists(), present, "{args:?}");
    }

    // `.claude` itself leads to the global folder.
    let folder = top.join("src/linked-folder");
    std::fs::create_dir_all(&folder).expect("project");
    std::os::unix::fs::symlink("../../.claude", folder.join(".claude")).expect("folder link");
    for args in [&["init", "--yes"][..], &["init", "--yes", "--force"][..]] {
        let before = outside_of(&top, &folder);
        let out = nika(&top, &folder, args);
        let text = transcript(&out);
        assert_eq!(out.status.code(), Some(3), "{args:?}\n{text}");
        assert!(
            text.lines().any(|line| line.starts_with(
                "✖ ./.claude/hooks-nika/check-on-edit.sh: its folder leads outside the project"
            )),
            "{args:?}\n{text}"
        );
        assert_eq!(
            outside_of(&top, &folder),
            before,
            "{args:?}: the global folder is untouched"
        );
        assert!(folder.join(".cursor/hooks.json").is_file(), "{text}");
    }
}

/// The real binary: `.claude` leads out of the project to a link that leads
/// back in (`.claude -> <top>/hop`, `hop -> <project>/agents-claude`). The
/// hop outside belongs to whoever owns that folder and can be turned
/// elsewhere between init's look and its write, so init refuses the way
/// where it leaves, `--force` included, and writes nothing in the folder the
/// way comes back to.
#[test]
fn init_refuses_a_folder_way_out_and_back_in() {
    let home = tempfile::tempdir().expect("isolated home");
    let top = home.path().canonicalize().expect("canonical home");
    let project = top.join("src/hop");
    let inner = project.join("agents-claude");
    std::fs::create_dir_all(&inner).expect("project");
    std::os::unix::fs::symlink(&inner, top.join("hop")).expect("hop back in");
    std::os::unix::fs::symlink(top.join("hop"), project.join(".claude")).expect("out");
    for args in [&["init", "--yes"][..], &["init", "--yes", "--force"][..]] {
        let before = outside_of(&top, &project);
        let out = nika(&top, &project, args);
        let text = transcript(&out);
        assert_eq!(out.status.code(), Some(3), "{args:?}\n{text}");
        assert!(
            text.lines().any(|line| line.starts_with(
                "✖ ./.claude/hooks-nika/check-on-edit.sh: its folder leads outside the project"
            )),
            "{args:?}\n{text}"
        );
        assert_eq!(
            std::fs::read_dir(&inner).expect("list").count(),
            0,
            "{args:?}: nothing written where the way comes back\n{text}"
        );
        assert_eq!(outside_of(&top, &project), before, "{args:?}");
        assert!(project.join(".cursor/hooks.json").is_file(), "{text}");
    }
}
