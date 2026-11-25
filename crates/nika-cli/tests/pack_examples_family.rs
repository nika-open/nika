// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>
#![allow(clippy::expect_used, clippy::panic)]
// Same carve-out as check_run_equivalence: this suite's WHOLE JOB is the
// real binary, and the per-entry lines + the ran/green/known-fail/skipped
// tally are the suite's DELIVERABLE (the law: a skip is printed and
// counted, never silent).
#![allow(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    clippy::print_stdout
)]

//! The examples FAMILY traversal — every shipped example is LAUNCHED, not
//! just listed.
//!
//! `pack_templates_family.rs` is the sibling for skeletons. The R1 corpus
//! sweep (2026-08-25) measured the dead
//! angle: 50 of 53 examples were launched by NO gate — and 8 of them were
//! red through the showroom door itself, invisible because nothing ran
//! them. A listing test (`verbs_static.rs` proves `try` NAMES each slug)
//! is not a run; `nika check` green proves the audit, never the run.
//!
//! Every row of `pack_examples_family.manifest.yaml` (checked in beside
//! this file — the pack tree itself is spec-pinned, so the manifest can
//! not live inside it) drives the real binary, hermetically:
//!
//! - `door: try` (default) goes through `nika try <slug>` — the shipped
//!   door, mock seat by default (its fetches stay real · #1605), fixtures
//!   materialized by the door.
//!   C01 shipped `--answer` on this door; a gated row may stay on `try`.
//! - `door: run` stages a scratch cwd per the row's `kit:` and goes through
//!   `nika run --no-progress --max-cost-usd 0.01` — for rows that need a
//!   workspace kit the showroom does not stage.
//! - `HOME`/`TMPDIR` are isolated per entry; inherited `NIKA_*`, provider
//!   keys and workflow-referenced secret vars are scrubbed, and the only
//!   model the gate may name is `mock/echo`.
//! - A `known_fail` row is an XFAIL: on the listed OSes the example RUNS
//!   and must stay red — a green there means the wound healed and the gate
//!   refuses to keep lying about it. A `needs:` row is the honest skip:
//!   launched never, printed and counted always.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

#[path = "support/pack_radar_http.rs"]
mod radar_http;

/// The checked-in manifest, compiled in — the test and its contract are
/// one artifact (a manifest edit without the test is nothing; the
/// completeness law below makes the reverse drift a failure too).
const MANIFEST_YAML: &str = include_str!("pack_examples_family.manifest.yaml");

/// Per-entry run ceiling, seconds (worst measured in the sweep: 31 s).
const MAX_TIMEOUT_SECS: u64 = 120;

/// The one model this gate may name — zero keys, zero keychain. A row
/// naming anything else is a hermeticity breach. Network is NOT claimed
/// hermetic: a `door: run` row exercises the workflow's own declared
/// `permits.net` hosts (the sandbox bounds them to exactly those). The weekly
/// radar substitutes a declared local feed in its staged copy; the real fetch,
/// gated graph and final write still run, with request and data-flow witnesses.
const HERMETIC_MODEL: &str = "mock/echo";

// ── The manifest schema ───────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// class → issue token (the tracker row the known-fail is bound to).
    issues: BTreeMap<String, String>,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    slug: String,
    /// `try` (the shipped showroom door) or `run` (staged scratch cwd).
    #[serde(default)]
    door: Door,
    /// What the harness stages for `door: run`.
    #[serde(default)]
    kit: Vec<Kit>,
    /// Pre-seeded gate answers (`--answer approve=true`).
    #[serde(default)]
    answers: BTreeMap<String, String>,
    /// Workflow inputs (`--var k=v`).
    #[serde(default)]
    vars: BTreeMap<String, String>,
    /// Only ever `mock/echo` — enforced below.
    model: Option<String>,
    /// Seconds; ceiling [`MAX_TIMEOUT_SECS`].
    timeout: Option<u64>,
    expect: Expect,
    /// XFAIL: on these OSes the run must stay red (green = healed = fail).
    known_fail: Option<KnownFail>,
    /// The honest skip — never launched, always counted.
    needs: Option<Need>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Door {
    #[default]
    Try,
    Run,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Kit {
    Fixtures,
    GitRepo,
    CargoProject,
    SpecRepo,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    rc: Option<i32>,
    finding: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnownFail {
    /// Platform names — the sweep's `darwin` (Rust's `macos` `target_os`)
    /// · `linux`.
    on: Vec<String>,
    /// The measured failure class (a key of `issues:`).
    class: String,
    /// The typed code the red run must carry (absent = any red holds).
    finding: Option<String>,
    /// The tracker token — must be the `issues:` value for `class`.
    issue: String,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Need {
    /// Benches/serves real local models the mock plane cannot impersonate.
    OllamaModel,
    /// Drives a repo shape this harness does not stage.
    Manual,
}

fn manifest() -> Manifest {
    let m: Manifest =
        serde_yaml_bw::from_str(MANIFEST_YAML).expect("the checked-in manifest parses");
    validate(&m);
    m
}

/// The manifest's internal laws — every one fails HERE, at schema level,
/// never as a surprise mid-traversal.
fn validate(m: &Manifest) {
    assert!(!m.entries.is_empty(), "the manifest names examples");
    for e in &m.entries {
        assert!(
            e.expect.rc.is_some() || e.expect.finding.is_some(),
            "`{}`: expect names an rc, a finding, or both",
            e.slug
        );
        assert!(
            e.expect.finding.is_none() || e.expect.rc != Some(0),
            "`{}`: a finding is a refusal, never rc 0",
            e.slug
        );
        if let Some(model) = &e.model {
            assert_eq!(
                model, HERMETIC_MODEL,
                "`{}`: the gate names no model but {HERMETIC_MODEL}",
                e.slug
            );
        }
        if let Some(t) = e.timeout {
            assert!(
                t <= MAX_TIMEOUT_SECS,
                "`{}`: timeout {t}s exceeds the {MAX_TIMEOUT_SECS}s ceiling",
                e.slug
            );
        }
        assert!(
            e.known_fail.is_none() || e.needs.is_none(),
            "`{}`: known_fail and needs are mutually exclusive — one row, one honesty",
            e.slug
        );
        if e.door == Door::Try {
            assert!(
                e.kit.is_empty(),
                "`{}`: `try` stages its own room — kits belong to `door: run`",
                e.slug
            );
        }
        if let Some(kf) = &e.known_fail {
            assert!(!kf.on.is_empty(), "`{}`: known_fail names its OSes", e.slug);
            match m.issues.get(&kf.class) {
                Some(token) => assert_eq!(
                    &kf.issue, token,
                    "`{}`: the issue token is the `issues:` entry for its class",
                    e.slug
                ),
                None => panic!(
                    "`{}`: class `{}` has no `issues:` token — a known-fail with no tracker is a silent skip",
                    e.slug, kf.class
                ),
            }
        }
    }
}

// ── The hermetic launch ─────────────────────────────────────────────────────

/// One entry's sandbox: per-slug scratch holding the run cwd, the isolated
/// HOME, the per-door TMPDIR, and the captured output logs.
struct Room {
    root: PathBuf,
    cwd: PathBuf,
}

impl Room {
    fn enter(slug: &str) -> Self {
        let stem = slug.replace('/', "-");
        let root = std::env::temp_dir().join(format!(
            "nika-pack-examples-family-{}-{stem}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = root.join("cwd");
        std::fs::create_dir_all(cwd.join("home")).expect("scratch home");
        std::fs::create_dir_all(root.join("tmp")).expect("scratch tmp");
        Room { root, cwd }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

/// Every `secrets.*.key` a shipped example reads, from its parsed
/// `secrets:` block. A child that inherited a live one could make real
/// egress or spend (a QR Code AI key buys art codes), so the scrub reads
/// the corpus instead of a list kept by hand, which missed seven keys when
/// the recipes landed. Provider keys also ride the `*_API_KEY` sweep.
fn corpus_secret_keys() -> &'static BTreeSet<String> {
    static KEYS: std::sync::LazyLock<BTreeSet<String>> = std::sync::LazyLock::new(|| {
        let mut keys = BTreeSet::new();
        for slug in nika_pack::example_slugs() {
            let body = nika_pack::example(&slug).expect("a listed example has a body");
            let wf = nika_schema::parse(
                body,
                nika_schema::FileId::new(0),
                nika_schema::ParseMode::Lenient,
            )
            .unwrap_or_else(|e| panic!("`{slug}` must parse to name its secrets: {e}"));
            keys.extend(wf.secrets.iter().map(|(_, s)| s.value.key.clone()));
        }
        keys
    });
    &KEYS
}

/// The child environment law: the operator's exports never leak into a
/// corpus run — `NIKA_*`, provider keys, and the secret vars the corpus
/// references are all scrubbed; HOME and TMPDIR are the entry's own;
/// PATH survives (an `exec` example must find the same programs the
/// operator does).
fn hermetic(cmd: &mut Command, room: &Room) {
    let secrets = corpus_secret_keys();
    for (key, _) in std::env::vars_os() {
        let k = key.to_string_lossy();
        if k.starts_with("NIKA_") || k.ends_with("_API_KEY") || secrets.contains(k.as_ref()) {
            cmd.env_remove(&key);
        }
    }
    cmd.env("HOME", room.cwd.join("home"))
        .env("TMPDIR", room.path("tmp"))
        .env("NIKA_KEYCHAIN", "off")
        .env("NO_COLOR", "1")
        .current_dir(&room.cwd);
}

struct Outcome {
    code: Option<i32>,
    timed_out: bool,
    out: String,
    err: String,
    elapsed: Duration,
}

fn nika() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nika"))
}

/// Spawn and wait with the row's ceiling — stdout/stderr land in FILES
/// (a piping poll loop deadlocks the day an example outtalks the pipe
/// buffer; the file form has no such size).
fn launch(
    mut cmd: Command,
    room: &Room,
    timeout: u64,
    mut feed: Option<&mut radar_http::RadarHttp>,
) -> Outcome {
    let out_path = room.path("out.log");
    let err_path = room.path("err.log");
    let out_file = std::fs::File::create(&out_path).expect("out log");
    let err_file = std::fs::File::create(&err_path).expect("err log");
    cmd.stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file));
    hermetic(&mut cmd, room);
    let start = Instant::now();
    let deadline = start + Duration::from_secs(timeout);
    let mut child = cmd.spawn().expect("the nika binary spawns");
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if start.elapsed() > Duration::from_secs(timeout) => {
                let _ = child.kill();
                timed_out = true;
                break child.wait().ok().and_then(|s| s.code());
            }
            Ok(None) => {
                if let Some(feed) = feed.as_deref_mut() {
                    feed.poll(deadline);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("wait on the corpus child: {e}"),
        }
    };
    let read = |p: &Path| {
        let mut s = String::new();
        let _ = std::fs::File::open(p)
            .expect("log reopens")
            .read_to_string(&mut s);
        s
    };
    Outcome {
        code,
        timed_out,
        out: read(&out_path),
        err: read(&err_path),
        elapsed: start.elapsed(),
    }
}

/// `door: run` staging — the kits are the declarative answer to the sweep's
/// FIXTURE-CWD / repo-shape classes.
fn stage_run_room(room: &Room, entry: &Entry, body: &str) -> PathBuf {
    let stem = entry.slug.replace('/', "-");
    let path = room.cwd.join(format!("{stem}.nika"));
    std::fs::write(&path, body).expect("plant the example");
    for kit in &entry.kit {
        match kit {
            // The same materializer `try`/`new` use — ONE implementation
            // for every taking door (nika-onboard's own law).
            Kit::Fixtures => {
                nika_onboard::fixtures::materialize(body, &path).expect("the fixtures kit stages");
            }
            Kit::GitRepo => stage_git_repo(&room.cwd),
            Kit::CargoProject => {
                std::fs::create_dir_all(room.cwd.join("src")).expect("cargo src");
                std::fs::write(
                    room.cwd.join("Cargo.toml"),
                    "[package]\nname = \"x\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
                )
                .expect("cargo manifest");
                std::fs::write(room.cwd.join("src/main.rs"), "fn main() {}\n").expect("cargo main");
            }
            Kit::SpecRepo => {
                std::fs::create_dir_all(room.cwd.join("schemas")).expect("spec schemas");
                std::fs::write(room.cwd.join("VERSION"), "0.114.0\n").expect("spec VERSION");
                std::fs::write(
                    room.cwd.join("CHANGELOG.md"),
                    "# Changelog\n\n## [Unreleased]\n",
                )
                .expect("spec CHANGELOG");
                std::fs::write(
                    room.cwd.join("schemas/workflow.schema.json"),
                    nika_pack::schema_json(),
                )
                .expect("spec schema");
            }
        }
    }
    path
}

fn stage_git_repo(dir: &Path) {
    let git = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(dir)
            .env("HOME", dir.join("home"))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // An inherited GIT_DIR/GIT_WORK_TREE would aim the kit's commits
        // at the CALLER's checkout — scrub every GIT_* the runner exports.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(&key);
            }
        }
        let status = cmd.status().expect("git is on PATH (CI runners ship it)");
        assert!(status.success(), "the git-repo kit stages: git {args:?}");
    };
    std::fs::create_dir_all(dir.join("src")).expect("kit src dir");
    std::fs::write(dir.join("src/change.txt"), "the git-repo kit's one file\n")
        .expect("kit src file");
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.email=gate@example.com",
        "-c",
        "user.name=gate",
        "commit",
        "-qm",
        "init",
    ]);
}

/// Assemble the row's command — `try` speaks the showroom dialect (no
/// `--model`: the mock rehearsal IS the default seat), `run`
/// carries the trio plus the row's model/answers/vars.
fn entry_command(entry: &Entry, room: &Room, body: &str) -> Command {
    let mut cmd = nika();
    match entry.door {
        Door::Try => {
            cmd.arg("try")
                .arg(&entry.slug)
                .args(["--no-progress", "--max-cost-usd", "0.01"]);
            for (task, value) in &entry.answers {
                cmd.args(["--answer", &format!("{task}={value}")]);
            }
        }
        Door::Run => {
            let path = stage_run_room(room, entry, body);
            cmd.arg("run")
                .arg(&path)
                .args(["--no-progress", "--max-cost-usd", "0.01"]);
            if let Some(model) = &entry.model {
                cmd.args(["--model", model]);
            }
            for (task, value) in &entry.answers {
                cmd.args(["--answer", &format!("{task}={value}")]);
            }
        }
    }
    for (key, value) in &entry.vars {
        cmd.args(["--var", &format!("{key}={value}")]);
    }
    cmd
}

/// Refuses to launch a row that says yes to a local server while a program
/// answers on its port: comfyui-image-chain queues renders on
/// 127.0.0.1:8188, a constant of the file, and a running `ComfyUI` would
/// take them. With nothing listening, the queue meets a closed port and
/// the recipe recovers. A connect, not a bind: a server bound to every
/// address still answers on the loopback.
fn refuse_if_a_local_server_answers(port: u16) {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    assert!(
        std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err(),
        "a program answers on 127.0.0.1:{port}: the yes row would reach it. \
         Stop that server and run the family again."
    );
}

fn launch_entry(entry: &Entry, room: &Room, body: &str) -> (Outcome, Option<String>) {
    if entry.slug == "comfyui-image-chain" {
        refuse_if_a_local_server_answers(8188);
    }
    let mut feed = (entry.slug == "snippets/weekly-radar").then(radar_http::RadarHttp::new);
    let staged = feed.as_ref().map(|feed| {
        feed.staged_body(body)
            .expect("reviewed local feed transport")
    });
    let cmd = entry_command(entry, room, staged.as_deref().unwrap_or(body));
    if feed.is_some() {
        assert!(
            !room.cwd.join("radar/radar.md").exists(),
            "the result must be produced by the workflow"
        );
    }
    let outcome = launch(
        cmd,
        room,
        entry.timeout.unwrap_or(MAX_TIMEOUT_SECS),
        feed.as_mut(),
    );
    let feed_failure = feed.as_ref().and_then(|feed| feed.verify(&room.cwd).err());
    (outcome, feed_failure)
}

// ── The verdicts ────────────────────────────────────────────────────────────

fn tail(text: &str) -> String {
    text.lines()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Judge one launched row against its expectation. `None` = green; the
/// string is the failure's evidence.
fn judge(entry: &Entry, o: &Outcome) -> Option<String> {
    if o.timed_out {
        return Some(format!(
            "timed out past {}s",
            entry.timeout.unwrap_or(MAX_TIMEOUT_SECS)
        ));
    }
    match (&entry.expect.rc, &entry.expect.finding) {
        (Some(rc), None) => {
            if o.code == Some(*rc) {
                None
            } else {
                Some(format!("rc {:?} ≠ expected {rc}", o.code))
            }
        }
        (rc, Some(code)) => {
            if o.code == Some(0) {
                Some(format!("rc 0 but the row expects the finding {code}"))
            } else if rc.is_some_and(|rc| o.code != Some(rc)) {
                Some(format!(
                    "rc {:?} ≠ expected {} for the finding {code}",
                    o.code,
                    rc.unwrap_or_default()
                ))
            } else if o.out.contains(code.as_str()) || o.err.contains(code.as_str()) {
                None
            } else {
                Some(format!("rc {:?} without the expected {code}", o.code))
            }
        }
        (None, None) => Some("expect carries neither rc nor finding".to_owned()),
    }
}

/// #1605 · a `door: try` rehearsal names, before its run, the
/// hosts its example's `permits.net.http` declares — the mock seat replaces
/// the model, not the network — and names none when it declares none.
/// `None` = the try card told the truth.
fn reach_failure(entry: &Entry, body: &str, o: &Outcome) -> Option<String> {
    if entry.door != Door::Try {
        return None;
    }
    let wf = nika_schema::parse(
        body,
        nika_schema::FileId::new(0),
        nika_schema::ParseMode::Strict,
    )
    .ok()?;
    let hosts = wf
        .permits
        .as_ref()
        .and_then(|permits| permits.value.net.as_ref())
        .map(|net| net.http.join(" · "))
        .filter(|hosts| !hosts.is_empty());
    let named: Vec<&str> = o
        .err
        .lines()
        .filter(|line| line.starts_with("may reach: "))
        .collect();
    match hosts {
        Some(hosts) => {
            let want = format!("may reach: {hosts} — the mock replaces the model, not the network");
            (named != [want.as_str()]).then(|| {
                format!("the try card must name its hosts once: want {want:?}, got {named:?}")
            })
        }
        None => (!named.is_empty())
            .then(|| format!("the try card names hosts the example does not declare: {named:?}")),
    }
}

// ── The gates ───────────────────────────────────────────────────────────────

/// The completeness ratchet: the manifest and the pack are the SAME set —
/// an example with no row never re-enters the dead angle, and a row naming
/// no example fails instead of rotting.
#[test]
fn the_manifest_covers_every_shipped_example() {
    let m = manifest();
    let release = m
        .entries
        .iter()
        .find(|e| e.slug == "release-train")
        .expect("release train row");
    assert!(release.door == Door::Try);
    assert!(release.kit.is_empty() && release.vars.is_empty() && release.answers.is_empty());
    assert!(release.needs.is_none() && release.known_fail.is_none());
    assert_eq!(release.expect.rc, Some(4), "bare try reaches the conductor");
    let shipped: BTreeSet<String> = nika_pack::example_slugs().into_iter().collect();
    let named: BTreeSet<String> = m.entries.iter().map(|e| e.slug.clone()).collect();
    assert_eq!(
        named.len(),
        m.entries.len(),
        "a slug is named twice in the manifest"
    );
    assert_eq!(
        named,
        shipped,
        "manifest Δ pack — missing rows: {:?} · stray rows: {:?}",
        shipped.difference(&named).collect::<Vec<_>>(),
        named.difference(&shipped).collect::<Vec<_>>(),
    );
    for e in &m.entries {
        assert!(
            nika_pack::example(&e.slug).is_some(),
            "`{}` resolves in the embedded pack",
            e.slug
        );
    }
}

/// The traversal: every example launched per its row, every skip explicit
/// and counted, the tally printed — ran / green / known-fail / skipped.
#[test]
fn every_shipped_example_runs_per_its_manifest_row() {
    let m = manifest();
    // The manifest speaks the sweep's platform names: `darwin` for the OS
    // Rust calls `macos` (target_os / env::consts::OS), the rest verbatim.
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    // ran · green · typed refusal · known-fail · skipped
    let mut tally = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();

    for entry in &m.entries {
        let row = &entry.slug;
        if let Some(need) = &entry.needs {
            tally.4 += 1;
            println!("SKIP       {row} (needs: {})", need_name(*need));
            continue;
        }
        let body = nika_pack::example(row).expect("completeness gate ran first");
        let room = Room::enter(row);
        let (outcome, feed_failure) = launch_entry(entry, &room, body);
        tally.0 += 1;

        match entry
            .known_fail
            .as_ref()
            .filter(|kf| kf.on.iter().any(|o| o == os))
        {
            // The XFAIL leg: the run must STAY red on this OS, carrying the
            // class's finding when the row names one. Green = healed = the
            // manifest is now lying — fail and ask for the promotion.
            Some(kf) => {
                let healed = !outcome.timed_out && outcome.code == Some(0);
                let finding_seen = kf.finding.as_ref().is_none_or(|code| {
                    outcome.out.contains(code.as_str()) || outcome.err.contains(code.as_str())
                });
                if healed {
                    failures.push(format!(
                        "{row}: known-fail ({}) came back GREEN — the wound healed; \
                         promote the row to a plain expectation and close {}",
                        kf.class, kf.issue
                    ));
                } else if !finding_seen {
                    failures.push(format!(
                        "{row}: red but without {} — the class moved:\n{}\n{}",
                        kf.finding.as_deref().unwrap_or("?"),
                        tail(&outcome.out),
                        tail(&outcome.err)
                    ));
                } else {
                    tally.3 += 1;
                    println!(
                        "KNOWN-FAIL {row} ({} · {} · {} · {:.0?})",
                        kf.class, kf.issue, os, outcome.elapsed
                    );
                }
            }
            None => match judge(entry, &outcome)
                .or(feed_failure)
                .or_else(|| reach_failure(entry, body, &outcome))
            {
                None => {
                    if entry.expect.finding.is_some() {
                        tally.2 += 1;
                    } else {
                        tally.1 += 1;
                    }
                    println!("{}", verdict_line(entry, &outcome));
                }
                Some(why) => failures.push(format!(
                    "{row}: {why}\n--- stdout tail ---\n{}\n--- stderr tail ---\n{}",
                    tail(&outcome.out),
                    tail(&outcome.err)
                )),
            },
        }
    }

    println!(
        "pack examples family · {} entries: {} ran · {} green · {} typed refusal · {} known-fail · {} skipped",
        m.entries.len(),
        tally.0,
        tally.1,
        tally.2,
        tally.3,
        tally.4
    );
    assert_eq!(
        tally.0 + tally.4,
        m.entries.len(),
        "every row is either launched or counted as skipped — no silent third state"
    );
    assert!(
        failures.is_empty(),
        "{} example(s) failed their manifest row:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// The printed line of a row that met its expectation. A typed refusal ran
/// no task of its example: it is named and counted apart, never as green.
fn verdict_line(entry: &Entry, outcome: &Outcome) -> String {
    let door = match entry.door {
        Door::Try => "try",
        Door::Run => "run",
    };
    let rc = outcome.code.unwrap_or(-1);
    let elapsed = outcome.elapsed;
    if let Some(code) = &entry.expect.finding {
        format!(
            "REFUSED    {} ({door} · rc={rc} · {code} · {elapsed:.0?})",
            entry.slug
        )
    } else {
        format!(
            "GREEN      {} ({door} · rc={rc} · {elapsed:.0?})",
            entry.slug
        )
    }
}

fn need_name(need: Need) -> &'static str {
    match need {
        Need::OllamaModel => "ollama-model",
        Need::Manual => "manual",
    }
}

/// #1544: exercise the public try door, its staging and its actual run inputs.
/// No answer means no departure; a false answer completes without waiting.
#[test]
fn release_train_try_rehearses_with_isolated_inputs() {
    let room = Room::enter("release-train-regression");
    std::fs::write(room.cwd.join("VERSION"), "operator-version\n").expect("operator version");
    std::fs::write(room.cwd.join("CHANGELOG.md"), "operator notes\n").expect("operator notes");
    for (slug, args, expected) in [
        ("release-train", vec![], 4),
        ("release-train.nika", vec!["--answer", "conductor=false"], 0),
        ("release-train", vec!["--var", "version=9.9.9"], 1),
        ("release-train", vec!["--var", "hold_for=2s"], 4),
    ] {
        let mut cmd = nika();
        cmd.args(["try", slug, "--no-progress"]).args(&args);
        let outcome = launch(cmd, &room, 30, None);
        assert!(!outcome.timed_out, "{slug} {args:?} timed out");
        assert_eq!(
            outcome.code,
            Some(expected),
            "{slug} {args:?}: {}\n{}",
            outcome.out,
            outcome.err
        );
        assert!(!outcome.err.contains("NIKA-1708"), "{}", outcome.err);
        if expected == 1 {
            assert!(
                outcome.err.contains("NIKA-BUILTIN-ASSERT-001")
                    || outcome.out.contains("NIKA-BUILTIN-ASSERT-001"),
                "{}\n{}",
                outcome.out,
                outcome.err
            );
        }
    }
    assert_eq!(
        std::fs::read_to_string(room.cwd.join("VERSION")).expect("version"),
        "operator-version\n"
    );
    assert_eq!(
        std::fs::read_to_string(room.cwd.join("CHANGELOG.md")).expect("notes"),
        "operator notes\n"
    );
    assert_eq!(
        std::fs::read_dir(room.path("tmp"))
            .expect("try roots")
            .filter_map(Result::ok)
            .filter(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with("nika-rehearsal-"))
            .count(),
        0,
        "every invocation removes its temporary root, including a refused run"
    );
    assert!(!room.cwd.join("schemas").exists());
    assert!(!room.cwd.join(".nika").exists());
    let _ = std::fs::remove_dir_all(room.root);
}

/// The recipes whose first task is a confirm gate with no default: every
/// later task waits on its answer. Their manifest rows answer yes; this law
/// pins the other side, so a bare `try` pauses before any effect.
const ROOT_GATED: [&str; 7] = [
    "comfyui-image-chain",
    "feed-digest",
    "lead-enrichment",
    "listing-link-audit",
    "qr-art-forge",
    "refund-chaser",
    "seo-site-audit",
];

/// Unanswered, each root-gated recipe pauses at its gate (exit 4) and the
/// rehearsal leaves nothing behind: no output in the cwd, no `.nika`
/// journal, no temporary root.
#[test]
fn the_root_gated_recipes_pause_unanswered_and_write_nothing() {
    let m = manifest();
    for slug in ROOT_GATED {
        let row = m
            .entries
            .iter()
            .find(|e| e.slug == slug)
            .unwrap_or_else(|| panic!("`{slug}` has a manifest row"));
        assert_eq!(
            row.answers.get("approve").map(String::as_str),
            Some("true"),
            "`{slug}`: its row runs the answered side"
        );
        let room = Room::enter(&format!("unanswered-{slug}"));
        let mut cmd = nika();
        cmd.args(["try", slug, "--no-progress", "--max-cost-usd", "0.01"]);
        let outcome = launch(cmd, &room, 60, None);
        assert!(!outcome.timed_out, "{slug} timed out");
        assert_eq!(
            outcome.code,
            Some(4),
            "{slug}: an unanswered gate pauses\n{}\n{}",
            tail(&outcome.out),
            tail(&outcome.err)
        );
        assert!(
            outcome.out.contains("awaiting an answer for `approve`")
                || outcome.err.contains("awaiting an answer for `approve`"),
            "{slug}: the pause names its gate\n{}",
            tail(&outcome.out)
        );
        let left: Vec<String> = std::fs::read_dir(&room.cwd)
            .expect("the room cwd")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "home")
            .collect();
        assert!(
            left.is_empty(),
            "{slug}: the paused rehearsal wrote {left:?}"
        );
        let roots = std::fs::read_dir(room.path("tmp"))
            .expect("try roots")
            .filter_map(Result::ok)
            .count();
        assert_eq!(
            roots, 0,
            "{slug}: the paused rehearsal kept its temporary root"
        );
        let _ = std::fs::remove_dir_all(room.root);
    }
}

/// A row that expects a typed refusal ran no task of its example, so the
/// file's offline proof is `nika check`: it must come back clean.
#[test]
fn every_refused_row_checks_clean() {
    let m = manifest();
    let refused: Vec<&Entry> = m
        .entries
        .iter()
        .filter(|e| e.expect.finding.is_some())
        .collect();
    assert!(
        !refused.is_empty(),
        "the manifest pins at least one refusal"
    );
    for entry in refused {
        let body = nika_pack::example(&entry.slug).expect("a shipped example");
        let room = Room::enter(&format!("check-{}", entry.slug));
        let file = room
            .cwd
            .join(format!("{}.nika", entry.slug.replace('/', "-")));
        std::fs::write(&file, body).expect("the example body");
        let mut cmd = nika();
        cmd.arg("check").arg(&file);
        let outcome = launch(cmd, &room, 60, None);
        assert_eq!(
            outcome.code,
            Some(0),
            "`{}`: nika check must pass\n{}\n{}",
            entry.slug,
            tail(&outcome.out),
            tail(&outcome.err)
        );
        let _ = std::fs::remove_dir_all(room.root);
    }
}

/// The scrub list is read from the corpus, so it can never come back empty
/// or lose a key a recipe reads.
#[test]
fn the_scrub_list_names_every_corpus_secret() {
    let keys = corpus_secret_keys();
    for key in [
        "QRCAI_API",
        "ALERTS_WEBHOOK_URL",
        "ONCALL_WEBHOOK_URL",
        "MAIL_API_TOKEN",
    ] {
        assert!(
            keys.contains(key),
            "`{key}` is read by a shipped example: {keys:?}"
        );
    }
}
