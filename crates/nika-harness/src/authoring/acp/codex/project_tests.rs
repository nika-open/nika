// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The project configuration a Codex session would see, on temporary trees: the session's
//! directory and each parent up to the repository root count, a scratch is judged like any
//! directory, a link counts without being followed, what lies above the root does not, a lookup
//! that fails counts, a `.git` that does not resolve ends nothing, a path with `..` is refused,
//! and the seat's own Codex home is not a project's. Each tree carries its own repository root
//! where the case allows, so what lies above the temporary directory does not decide.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;

/// `dir` made, with its parents.
fn made(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("directory");
    dir.to_path_buf()
}

/// A `.codex` directory holding a `config.toml` in `dir`; returns that file.
fn codex_config(dir: &Path) -> PathBuf {
    let config = made(&dir.join(".codex")).join("config.toml");
    std::fs::write(&config, "[mcp_servers.project]\ncommand = \"x\"\n").expect("config");
    config
}

/// `dir` marked as a repository root, the way Codex finds one.
fn repository(dir: &Path) -> PathBuf {
    made(&made(dir).join(".git"));
    dir.to_path_buf()
}

fn no_env() -> BTreeMap<String, String> {
    BTreeMap::new()
}

/// The paths of the entries found, in the order the walk meets them.
fn paths(found: Vec<Entry>) -> Vec<PathBuf> {
    found.into_iter().map(|entry| entry.path).collect()
}

/// The refusal for `named` (each entry as the refusal names it, joined).
fn refusal(named: &str) -> String {
    format!(
        "Codex would apply the project configuration {named} to this session: codex-acp \
         trusts the session's directory, and Codex reads the `.codex` entry of that \
         directory and of each parent up to the repository root; no session was opened; \
         no fallback (move the entry out of the project, or run the task from a directory \
         that has none)"
    )
}

/// One entry as the refusal names it.
fn named(path: &Path) -> String {
    format!("`{}`", path.display())
}

/// A session directory holding `.codex/config.toml` does not open, and the refusal names that
/// exact file and says that Codex would apply it.
#[test]
fn a_codex_config_in_the_session_directory_refuses_naming_the_file() {
    let tree = tempfile::tempdir().expect("tree");
    let project = repository(&tree.path().join("project"));
    let config = codex_config(&project);
    assert_eq!(
        paths(project_entries(&project, None)),
        std::slice::from_ref(&config)
    );
    assert_eq!(
        project_config(&project, &no_env()),
        Err(refusal(&named(&config)))
    );
}

/// A `.codex` in a parent below the repository root counts, the root's own included, named in
/// the order the walk meets them; a `.codex` directory without `config.toml` is named itself.
#[test]
fn a_codex_entry_in_a_parent_up_to_the_repository_root_counts() {
    let tree = tempfile::tempdir().expect("tree");
    let root = repository(&tree.path().join("repo"));
    let root_config = codex_config(&root);
    let middle = made(&root.join("services/api/.codex"));
    let session = made(&root.join("services/api/handlers"));
    assert_eq!(
        paths(project_entries(&session, None)),
        [middle.clone(), root_config.clone()]
    );
    let both = format!("{} · {}", named(&middle), named(&root_config));
    assert_eq!(project_config(&session, &no_env()), Err(refusal(&both)));
}

/// A scratch the engine creates inside a repository whose root holds a `.codex` is judged like
/// any directory: the root's file is named. The same scratch in a repository without one opens.
#[test]
fn a_scratch_inside_a_repository_is_judged_like_any_directory() {
    let tree = tempfile::tempdir().expect("tree");
    let around = repository(&tree.path().join("checkout"));
    let config = codex_config(&around);
    let scratch = made(&around.join("tmp/.tmpA1b2C3"));
    assert_eq!(
        project_config(&scratch, &no_env()),
        Err(refusal(&named(&config)))
    );
    let clean = repository(&tree.path().join("clean"));
    let scratch = made(&clean.join("tmp/.tmpA1b2C3"));
    assert_eq!(project_config(&scratch, &no_env()), Ok(()));
}

/// A `.codex` that is a link counts as present and is never followed to decide: named as the
/// link, whether it leads to a directory outside the tree that holds a `config.toml` or to
/// nothing at all.
#[cfg(unix)]
#[test]
fn a_codex_link_counts_and_is_never_followed() {
    let tree = tempfile::tempdir().expect("tree");
    let outside = made(&tree.path().join("elsewhere"));
    let _ = codex_config(&outside);
    let project = repository(&tree.path().join("project"));
    let link = project.join(".codex");
    std::os::unix::fs::symlink(outside.join(".codex"), &link).expect("link");
    assert_eq!(paths(project_entries(&project, None)), [link]);
    let dangling = repository(&tree.path().join("dangling"));
    let broken = dangling.join(".codex");
    std::os::unix::fs::symlink(tree.path().join("gone"), &broken).expect("dangling link");
    assert_eq!(
        project_config(&dangling, &no_env()),
        Err(refusal(&named(&broken)))
    );
}

/// A `.codex` above the repository root is not the project's: the walk stops at the first
/// directory whose `.git` resolves, a file (a linked worktree's) as well as a directory. Other
/// names that only look like it (`.codex.bak`, `codex`) are no entry at any depth.
#[test]
fn a_codex_entry_above_the_repository_root_does_not_count() {
    let tree = tempfile::tempdir().expect("tree");
    let outer = repository(&tree.path().join("studio"));
    let _ = codex_config(&outer);
    let repo = repository(&outer.join("engine"));
    let session = made(&repo.join("crates/core"));
    std::fs::write(repo.join(".codex.bak"), "x").expect("decoy");
    made(&repo.join("crates/codex"));
    assert_eq!(paths(project_entries(&session, None)), [] as [PathBuf; 0]);
    assert_eq!(project_config(&session, &no_env()), Ok(()));
    let worktree = made(&outer.join("worktree"));
    std::fs::write(
        worktree.join(".git"),
        "gitdir: ../engine/.git/worktrees/w\n",
    )
    .expect("file");
    assert_eq!(project_config(&worktree, &no_env()), Ok(()));
}

/// With no `.git` anywhere above it, a session is looked at alone: a `.codex` in its parent is
/// not counted, its own is. The case needs a temporary directory outside any repository, and
/// says so when it is not.
#[test]
fn without_a_repository_only_the_session_directory_counts() {
    let tree = tempfile::tempdir().expect("tree");
    // The walk's own lookup: any `.git` entry, a link that leads nowhere included.
    let above =
        (tree.path().ancestors()).find(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok());
    assert_eq!(
        above, None,
        "this case needs a temporary directory outside any repository"
    );
    let parent = made(&tree.path().join("loose"));
    let session = made(&parent.join("notes"));
    assert_eq!(project_config(&session, &no_env()), Ok(()));
    let _ = codex_config(&parent);
    assert_eq!(project_config(&session, &no_env()), Ok(()));
    let own = codex_config(&session);
    assert_eq!(paths(project_entries(&session, None)), [own]);
}

/// A `.git` that does not resolve (a dangling link) does not end the walk: it goes on to the
/// root whose `.git` resolves, whose `.codex` counts. With no `.git` that resolves, the walk
/// still reaches the outermost directory that holds a `.git` entry.
#[cfg(unix)]
#[test]
fn a_git_entry_that_does_not_resolve_does_not_end_the_walk() {
    let tree = tempfile::tempdir().expect("tree");
    let outer = repository(&tree.path().join("outer"));
    let config = codex_config(&outer);
    let inner = made(&outer.join("inner"));
    std::os::unix::fs::symlink(tree.path().join("gone"), inner.join(".git")).expect("link");
    let session = made(&inner.join("src"));
    assert_eq!(paths(project_entries(&session, None)), [config]);
    let lone = made(&tree.path().join("lone"));
    std::os::unix::fs::symlink(tree.path().join("gone"), lone.join(".git")).expect("link");
    let lone_codex = made(&lone.join(".codex"));
    let session = made(&lone.join("src"));
    assert!(
        paths(project_entries(&session, None)).contains(&lone_codex),
        "the outermost `.git` entry bounds the walk"
    );
}

/// A lookup that fails for another reason than an absent entry counts, named with its reason:
/// in a directory this process may not search, `.codex` cannot be looked at. A session rooted
/// at a file is no entry (not a directory), and its walk still reaches the root.
#[cfg(unix)]
#[test]
fn an_entry_that_cannot_be_looked_at_counts_with_its_reason() {
    use std::os::unix::fs::PermissionsExt as _;
    let tree = tempfile::tempdir().expect("tree");
    let repo = repository(&tree.path().join("repo"));
    let notes = repo.join("notes.txt");
    std::fs::write(&notes, "x").expect("file");
    assert_eq!(project_config(&notes, &no_env()), Ok(()));
    let locked = made(&repo.join("locked"));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).expect("mode");
    let searchable = std::fs::symlink_metadata(locked.join("probe"));
    let denied = matches!(&searchable, Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied);
    let judged = project_config(&locked, &no_env());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("mode");
    if !denied {
        // This user searches every directory (a superuser): the lookup cannot fail here, and
        // the walk finds no entry.
        assert_eq!(judged, Ok(()), "{searchable:?}");
        return;
    }
    let unread = format!(
        "`{}` (not readable here: permission denied)",
        locked.join(".codex").display()
    );
    assert_eq!(judged, Err(refusal(&unread)));
}

/// A session directory that holds `..` is refused, the walk could not judge it as Codex reads
/// it; `.` components are dropped, so no directory is looked at twice.
#[test]
fn a_session_directory_with_dot_dot_is_refused_and_dots_judge_nothing_twice() {
    let tree = tempfile::tempdir().expect("tree");
    let repo = repository(&tree.path().join("repo"));
    let config = codex_config(&repo);
    made(&repo.join("a"));
    let twisted = repo.join("a/../b");
    assert_eq!(
        project_config(&twisted, &no_env()),
        Err(format!(
            "the session directory `{}` is named with `..`, so the project configuration Codex \
             would apply to it cannot be judged; no session was opened; no fallback (name the \
             directory without `..`; a temporary directory takes its name from `TMPDIR`)",
            twisted.display()
        ))
    );
    let own = made(&repo.join("b/.codex"));
    let dotted = repo.join("b/./.");
    assert_eq!(paths(project_entries(&dotted, None)), [own, config]);
}

/// The seat's own Codex home (`CODEX_HOME`, else `$HOME/.codex`) is the user's configuration,
/// not a project's: a session rooted where it lives opens. Any other `.codex` still refuses:
/// one beside a home of another name, and one under a relative home, which names no directory
/// of its own.
#[test]
fn the_seats_own_codex_home_is_not_a_project() {
    let tree = tempfile::tempdir().expect("tree");
    let root = repository(&tree.path().join("users"));
    let home = made(&root.join("home"));
    let config = codex_config(&home);
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let by_home = BTreeMap::from([("HOME".to_owned(), text(&home))]);
    assert_eq!(project_config(&home, &by_home), Ok(()));
    let by_codex_home = BTreeMap::from([("CODEX_HOME".to_owned(), text(&home.join(".codex")))]);
    assert_eq!(project_config(&home, &by_codex_home), Ok(()));
    let elsewhere = BTreeMap::from([
        ("HOME".to_owned(), text(&home)),
        ("CODEX_HOME".to_owned(), text(&tree.path().join("codex"))),
    ]);
    assert_eq!(
        project_config(&home, &elsewhere),
        Err(refusal(&named(&config)))
    );
    let sibling = BTreeMap::from([("CODEX_HOME".to_owned(), text(&home.join("codex-home")))]);
    assert_eq!(
        project_config(&home, &sibling),
        Err(refusal(&named(&config))),
        "a home of another name beside the entry is not the entry"
    );
    let relative = BTreeMap::from([("CODEX_HOME".to_owned(), ".codex".to_owned())]);
    assert_eq!(
        project_config(&home, &relative),
        Err(refusal(&named(&config)))
    );
}

/// A home reached through a link is still the home, from either spelling; a project's `.codex`
/// that is a link to the home is a project entry, never followed.
#[cfg(unix)]
#[test]
fn a_home_reached_through_a_link_is_still_the_home() {
    let tree = tempfile::tempdir().expect("tree");
    let root = repository(&tree.path().join("users"));
    let real = made(&root.join("real"));
    let _ = codex_config(&real);
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&real, &alias).expect("alias");
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let by_alias = BTreeMap::from([("HOME".to_owned(), text(&alias))]);
    assert_eq!(project_config(&real, &by_alias), Ok(()));
    let by_real = BTreeMap::from([("HOME".to_owned(), text(&real))]);
    assert_eq!(project_config(&alias, &by_real), Ok(()));
    let project = made(&root.join("project"));
    let link = project.join(".codex");
    std::os::unix::fs::symlink(real.join(".codex"), &link).expect("link to the home");
    assert_eq!(
        project_config(&project, &by_real),
        Err(refusal(&named(&link)))
    );
}

/// A Codex home that is a link into a repository (`~/.codex` leading to a checkout's `.codex`)
/// is still the home where it leads: a session in that checkout opens, the entry being the
/// home itself. A project `.codex` that is a link counts wherever it leads, the home included.
#[cfg(unix)]
#[test]
fn a_codex_home_that_links_into_a_repository_is_still_the_home() {
    let tree = tempfile::tempdir().expect("tree");
    let dotfiles = repository(&tree.path().join("dotfiles"));
    let config = codex_config(&dotfiles);
    let home = made(&tree.path().join("home"));
    let codex_home = home.join(".codex");
    std::os::unix::fs::symlink(dotfiles.join(".codex"), &codex_home).expect("home link");
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let by_home = BTreeMap::from([("HOME".to_owned(), text(&home))]);
    assert_eq!(project_config(&dotfiles, &by_home), Ok(()));
    let by_codex_home = BTreeMap::from([("CODEX_HOME".to_owned(), text(&codex_home))]);
    assert_eq!(
        project_config(&made(&dotfiles.join("src")), &by_codex_home),
        Ok(())
    );
    let project = repository(&tree.path().join("project"));
    let link = project.join(".codex");
    std::os::unix::fs::symlink(dotfiles.join(".codex"), &link).expect("project link");
    assert_eq!(
        project_config(&project, &by_home),
        Err(refusal(&named(&link))),
        "a project link to the home's directory still counts"
    );
    let elsewhere = BTreeMap::from([("HOME".to_owned(), text(&tree.path().join("other")))]);
    assert_eq!(
        project_config(&dotfiles, &elsewhere),
        Err(refusal(&named(&config))),
        "with another home, the checkout's `.codex` is a project's"
    );
}

/// A Codex home that is not an absolute path names no directory of its own: it resolves against
/// the working directory, which for a task is the project's. It exempts nothing, also when it
/// leads to the project's own `.codex`, by `CODEX_HOME` or by `HOME`.
#[cfg(unix)]
#[test]
fn a_codex_home_that_is_not_absolute_exempts_nothing() {
    let tree = tempfile::tempdir().expect("tree");
    let project = repository(&tree.path().join("project"));
    let config = codex_config(&project);
    // A relative path from this process's directory to the project, read and never changed.
    let normal = |part: &std::path::Component<'_>| matches!(part, std::path::Component::Normal(_));
    let here = std::env::current_dir().expect("working directory");
    let target = std::fs::canonicalize(&project).expect("project");
    let mut relative = PathBuf::new();
    for _ in here.components().filter(normal) {
        relative.push("..");
    }
    relative.extend(target.components().filter(normal));
    assert!(relative.is_relative());
    assert_eq!(
        std::fs::canonicalize(relative.join(".codex")).ok(),
        Some(target.join(".codex")),
        "the relative path leads to the project's own entry"
    );
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let by_codex_home = BTreeMap::from([("CODEX_HOME".to_owned(), text(&relative.join(".codex")))]);
    assert_eq!(
        project_config(&project, &by_codex_home),
        Err(refusal(&named(&config)))
    );
    let by_home = BTreeMap::from([("HOME".to_owned(), text(&relative))]);
    assert_eq!(
        project_config(&project, &by_home),
        Err(refusal(&named(&config)))
    );
}
