// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used, clippy::disallowed_types)]

//! The operator's repair writes live grammar and converges on a second pass.
use std::process::Command;

fn assert_fix(before: &str, expected: &str) {
    assert!(
        nika_schema::parse(
            before,
            nika_schema::FileId::new(0),
            nika_schema::ParseMode::Strict
        )
        .is_err()
    );
    let room = tempfile::tempdir().expect("isolated room");
    let home = room.path().join("home");
    std::fs::create_dir(&home).expect("home");
    let path = room.path().join("legacy.nika");
    std::fs::write(&path, before).expect("fixture");
    let call = || {
        Command::new(env!("CARGO_BIN_EXE_nika"))
            .args(["check", "legacy.nika", "--fix"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("NIKA_KEYCHAIN", "off")
            .env("NO_COLOR", "1")
            .current_dir(room.path())
            .output()
            .expect("isolated CLI")
    };
    let first = call();
    assert!(first.status.success(), "{first:?}");
    let actual = std::fs::read_to_string(&path).expect("repaired file");
    assert_eq!(actual, expected);
    nika_schema::parse(
        &actual,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .expect("live grammar");
    let second = call();
    assert!(second.status.success(), "{second:?}");
    assert_eq!(std::fs::read_to_string(&path).expect("second pass"), actual);
}

#[test]
fn fix_maps_invoke_params_to_args_without_changing_the_payload() {
    assert_fix(
        "nika: legacy\npermits: {tools: [nika:log]}\ntasks:\n  say:\n    invoke:\n      tool: nika:log\n      params: {message: 'argv: is data'} # keep\n",
        "nika: legacy\npermits: {tools: [nika:log]}\ntasks:\n  say:\n    invoke:\n      tool: nika:log\n      args: {message: 'argv: is data'} # keep\n",
    );
}

#[test]
fn fix_maps_exec_argv_to_command_without_inventing_a_shell() {
    assert_fix(
        "nika: legacy\npermits: {exec: [echo]}\ntasks:\n  say:\n    exec: {argv: [echo, 'hi; remains one argument']}\n",
        "nika: legacy\npermits: {exec: [echo]}\ntasks:\n  say:\n    exec: {command: [echo, 'hi; remains one argument']}\n",
    );
}

#[test]
fn fix_wraps_for_each_items_even_without_legacy_parallel_knobs() {
    assert_fix(
        "nika: legacy\nmodel: mock/echo\nconst: {items: [one, two]}\ntasks:\n  say:\n    for_each: ${{ const.items }} # keep\n    infer: {prompt: hi}\n",
        "nika: legacy\nmodel: mock/echo\nconst: {items: [one, two]}\ntasks:\n  say:\n    for_each:\n      items: ${{ const.items }} # keep\n    infer: {prompt: hi}\n",
    );
}

#[test]
fn fix_preserves_conflicting_keys_and_scalar_task_payloads_on_disk() {
    for body in [
        "  say:\n    invoke:\n      tool: nika:log\n      params: {message: first}\n# keep mapping open\n      args: {message: second}\n",
        "  say:\n    invoke: {tool: nika:log, params: {}, \"ar\\u0067s\": {}}\n",
        "  say:\n    exec: {argv: 'echo hi; touch sentinel'}\n",
        "  say: |\n    invoke:\n      params: {message: keep}\n",
    ] {
        let room = tempfile::tempdir().expect("isolated room");
        let home = room.path().join("home");
        std::fs::create_dir(&home).expect("home");
        let source = format!("nika: legacy\ntasks:\n{body}");
        let path = room.path().join("legacy.nika");
        std::fs::write(&path, &source).expect("fixture");
        let output = Command::new(env!("CARGO_BIN_EXE_nika"))
            .args(["check", "legacy.nika", "--fix"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("NIKA_KEYCHAIN", "off")
            .env("NO_COLOR", "1")
            .current_dir(room.path())
            .output()
            .expect("isolated CLI");
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged file"),
            source
        );
        assert!(!room.path().join("sentinel").exists());
    }
}

#[test]
fn fix_accepts_trailing_commas_in_legacy_verb_mappings() {
    assert_fix(
        "nika: legacy\npermits: {tools: [nika:log]}\ntasks:\n  say:\n    invoke: {tool: nika:log, params: {message: hi},}\n",
        "nika: legacy\npermits: {tools: [nika:log]}\ntasks:\n  say:\n    invoke: {tool: nika:log, args: {message: hi},}\n",
    );
    assert_fix(
        "nika: legacy\npermits: {exec: [echo]}\ntasks:\n  say:\n    exec: {argv: [echo],}\n",
        "nika: legacy\npermits: {exec: [echo]}\ntasks:\n  say:\n    exec: {command: [echo],}\n",
    );
}

/// The repair publishes through a new temporary file: a link waiting at the
/// fixed temporary name of earlier releases keeps pointing at a file outside
/// the project, that file keeps its bytes, and the workflow is repaired.
#[cfg(unix)]
#[test]
fn fix_never_writes_through_a_link_at_the_old_temporary_name() {
    let room = tempfile::tempdir().expect("isolated room");
    let home = room.path().join("home");
    std::fs::create_dir(&home).expect("home");
    let project = room.path().join("project");
    std::fs::create_dir(&project).expect("project");
    let outside = room.path().join("outside.rc");
    std::fs::write(&outside, "export KEEP=1\n").expect("outside file");
    let path = project.join("legacy.nika");
    std::fs::write(
        &path,
        "nika: legacy\npermits: {tools: [nika:log]}\ntasks:\n  say:\n    invoke: {tool: nika:log, params: {message: hi}}\n",
    )
    .expect("fixture");
    let planted = project.join(".nika-fix-tmp.legacy.nika");
    std::os::unix::fs::symlink(&outside, &planted).expect("planted link");
    let output = Command::new(env!("CARGO_BIN_EXE_nika"))
        .args(["check", "legacy.nika", "--fix"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("NIKA_KEYCHAIN", "off")
        .env("NO_COLOR", "1")
        .current_dir(&project)
        .output()
        .expect("isolated CLI");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(&outside).expect("outside file"),
        "export KEEP=1\n"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("repaired file"),
        "nika: legacy\npermits: {tools: [nika:log]}\ntasks:\n  say:\n    invoke: {tool: nika:log, args: {message: hi}}\n",
    );
    assert!(
        std::fs::symlink_metadata(&path)
            .expect("lstat")
            .file_type()
            .is_file()
    );
    assert_eq!(std::fs::read_link(&planted).expect("still a link"), outside);
    let mut left: Vec<String> = std::fs::read_dir(&project)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    left.sort();
    assert_eq!(left, [".nika-fix-tmp.legacy.nika", "legacy.nika"]);
}
