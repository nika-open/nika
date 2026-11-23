// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A deploy that kills the resident in the middle of a workflow (ADR-156):
//! the next resident resumes the job from the journal the killed one left.
//! SIGKILL targets only an owned child with a disposable world.

use std::io::{BufRead as _, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Child, Stdio};

use serde_json::json;

use super::*;
use crate::job::OnInterrupted;
use crate::server::production::ResidentExecutionBackend;
use crate::server::retry_tests::{
    TWO_TASKS, dropping, field, journal_lines, journal_of, kinds_and_statuses, names_task, pair,
    running_job, stored_jobs,
};

const WORKER: &str = "server::tests::shutdown::kill_resume::killed_worker";
const ROOT_ENV: &str = "NIKA_KILL_TEST_WORLD_ROOT";
const READY: &str = "NIKA_KILL_TEST_FIRST_TASK_JOURNALED";

struct Worker {
    child: Child,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Spawn the child resident over `world` and wait until its job's first
/// task is in the journal: the worker and the job's id.
fn start_worker(world: &TestWorld) -> (Worker, String) {
    // Test-owned process; the guard reaps it on every exit and panic path.
    #[allow(clippy::disallowed_types)]
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", WORKER, "--ignored", "--nocapture"])
        .env_clear()
        .env(ROOT_ENV, world.root.path())
        .env("HOME", world.root.path())
        .current_dir(world.root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the owned resident");
    let mut worker = Worker {
        child,
        reader: None,
    };
    let stdout = worker.child.stdout.take().expect("stdout pipe");
    let (ready, receiver) = std::sync::mpsc::sync_channel(1);
    // Keep draining after the marker so the child never blocks on a write.
    #[allow(clippy::disallowed_methods)]
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if let Some(id) = line.strip_prefix(READY) {
                        let _ = ready.try_send(id.trim().to_owned());
                    }
                }
                Err(_) => break,
            }
        }
    });
    worker.reader = Some(reader);
    let id = receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("the child journaled its first task");
    (worker, id)
}

#[test]
#[ignore = "child entry; invoked with an isolated world by the kill test"]
fn killed_worker() {
    #[allow(clippy::disallowed_methods)]
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("worker world"));
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("worker runtime")
        .block_on(async {
            // The parent owns the world, even after SIGKILL, when this
            // child's TempDir destructor cannot run.
            let mut world =
                TestWorld::from_tempdir(tempfile::tempdir_in(&root).expect("child scratch"));
            world.workflows = root.join("workflows");
            world.state = root.join("state");
            world.token = root.join("serve.token");
            let backend = Arc::new(ResidentExecutionBackend::new(&world.workflows));
            let server = world.start(backend, dropping(OnInterrupted::Off)).await;
            let body = json!({"workflow": "two.nika", "retry": {"on_interrupted": "resume"}});
            let (id, first) = running_job(&server, &body.to_string(), "killed-mid-run").await;
            let journals = world.workflows.join(nika_dap::store::TRACE_DIR);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            while tokio::time::Instant::now() < deadline {
                let lines = journal_of(&journals, &first)
                    .map_or_else(Vec::new, |path| journal_lines(&path));
                if names_task(&lines, "task_completed", "first") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            writeln!(std::io::stdout(), "{READY} {id}").expect("announce the job");
            std::io::stdout().flush().expect("flush marker");
            // `second` waits three seconds: the parent kills the process
            // long before it ends.
            std::future::pending::<()>().await;
        });
}

#[test]
fn a_resident_killed_in_the_middle_of_a_workflow_is_resumed_by_the_next_one() {
    let world = TestWorld::new();
    std::fs::write(world.workflows.join("two.nika"), TWO_TASKS).expect("workflow");
    let (mut worker, id) = start_worker(&world);
    worker.child.kill().expect("kill only the owned child");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = worker.child.try_wait().expect("child status") {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "SIGKILL stopped the child"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.signal(), Some(9));
    drop(worker);
    let record = stored_jobs(&world)[0]["record"].clone();
    assert_eq!(
        record["status"], "running",
        "a killed resident settles nothing: {record}"
    );
    let first_trace = record["trace_id"].clone();

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("restart runtime")
        .block_on(resumed_by_the_next_resident(&world, &id, &first_trace));
}

async fn resumed_by_the_next_resident(world: &TestWorld, id: &str, first_trace: &Value) {
    let journals = world.workflows.join(nika_dap::store::TRACE_DIR);
    let backend = Arc::new(ResidentExecutionBackend::new(&world.workflows));
    let server = world.start(backend, dropping(OnInterrupted::Off)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut job = Value::Null;
    while tokio::time::Instant::now() < deadline {
        job = server
            .request(&get_request(&format!("/v1/jobs/{id}")))
            .await
            .json();
        if job["status"] != "running" && job["status"] != "queued" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let lines = journal_of(&journals, &job).map_or_else(Vec::new, |path| journal_lines(&path));
    assert_eq!(job["status"], "succeeded", "{job} {lines:?}");
    assert_ne!(&job["trace_id"], first_trace);

    // The completed task is served from the killed attempt's journal.
    let boot = lines
        .iter()
        .find(|line| line["kind"] == "workflow_started")
        .expect("the boot record");
    assert_eq!(field(boot, "resumed_from"), Some(first_trace), "{boot}");
    assert!(names_task(&lines, "task_cache_hit", "first"), "{lines:?}");
    assert!(
        !names_task(&lines, "task_completed", "first"),
        "never run again"
    );
    assert!(names_task(&lines, "task_completed", "second"), "{lines:?}");

    // One job, two attempts; the lost one spent nothing it could not read.
    let read = server
        .request(&get_request(&format!("/v1/jobs/{id}/attempts")))
        .await
        .json();
    let views = read["attempts"].as_array().expect("attempts");
    assert_eq!(views.len(), 2, "{read}");
    assert_eq!(views[0]["status"], "interrupted", "{read}");
    assert_eq!(&views[0]["trace_id"], first_trace);
    assert_eq!(views[0]["spend"], json!({"usd": 0.0}), "{read}");
    assert_eq!(views[1]["mode"], "resume", "{read}");
    assert_eq!(&views[1]["resumed_from"], first_trace, "{read}");
    let frames = parse_sse_data(&server.request(&events_request(id, None)).await.body);
    assert_eq!(
        kinds_and_statuses(&frames),
        [
            pair("execution.queued", "queued"),
            pair("execution.started", "running"),
            pair("execution.requeued", "queued"),
            pair("execution.started", "running"),
            pair("execution.settled", "succeeded"),
        ],
        "{frames:?}"
    );
    assert_eq!(
        frames[2]["message"],
        "attempt 2 of 3 (resume): attempt 1 lost its owner"
    );

    // The killed attempt's journal is still verified on its own.
    let earlier = server
        .request(&get_request(&format!(
            "/v1/jobs/{id}/trace/verify?attempt=1"
        )))
        .await
        .json();
    assert_eq!(&earlier["trace_id"], first_trace, "{earlier}");
    assert_ne!(earlier["verdict"], "unavailable", "{earlier}");
    server.stop().await.expect("a clean stop");
}
