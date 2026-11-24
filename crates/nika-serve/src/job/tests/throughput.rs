// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! On-demand measurement of what one job costs the durable store as the
//! snapshot grows. Every store operation reloads, revalidates and rewrites
//! the whole snapshot, so the cost of one operation is linear in the number of
//! retained jobs. This prints the figures a retention bound is chosen from:
//!
//! ```sh
//! cargo test -p nika-serve --lib --locked -- --ignored --nocapture \
//!     job::tests::throughput
//! ```

use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use super::*;

/// The retained-job counts the figures are printed at.
const CHECKPOINTS: [usize; 4] = [250, 500, 1_000, 2_000];
/// How many lifecycles the figure at a checkpoint averages over.
const WINDOW: usize = 20;

#[test]
#[ignore = "measurement; run on demand with --nocapture"]
#[allow(clippy::disallowed_macros, clippy::print_stdout)]
fn one_job_lifecycle_costs_linearly_in_the_retained_snapshot() {
    let root = tempfile::tempdir().expect("root");
    let store = JobStore::open(root.path()).expect("store");
    let state = root.path().join("jobs/state.json");
    let last = CHECKPOINTS[CHECKPOINTS.len() - 1];
    let mut window = Vec::with_capacity(WINDOW);
    for index in 0..last {
        let started = Instant::now();
        let request = RequestDigest::from_bytes(Sha256::digest(index.to_le_bytes()).into());
        let record = admitted_record(
            store
                .create_or_replay_named(
                    key(&format!("measure-{index}")),
                    request.clone(),
                    usize::MAX,
                    "root.nika".to_owned(),
                )
                .expect("create"),
        );
        store
            .stamp_execution_identity(
                record.id(),
                format!("execution-{index}"),
                format!("trace-{index}"),
                request.as_str().to_owned(),
            )
            .expect("stamp");
        transition(&store, record.id(), JobStatus::Running);
        let receipt = JobReceipt::new(
            record.id().clone(),
            format!("execution-{index}"),
            format!("trace-{index}"),
            request.as_str().to_owned(),
            Some("measured-chain-head".to_owned()),
        )
        .expect("receipt");
        store
            .settle_with_events(
                record.id(),
                JobStatus::Succeeded,
                &[json!({"kind": "execution.settled", "status": "succeeded"})],
                None,
                Some(receipt),
            )
            .expect("settle");
        let lifecycle = started.elapsed();
        let read = Instant::now();
        store.get(record.id()).expect("read").expect("job");
        let read = read.elapsed();
        if window.len() == WINDOW {
            window.remove(0);
        }
        window.push((lifecycle, read));
        if CHECKPOINTS.contains(&(index + 1)) {
            let count = u32::try_from(window.len()).expect("window");
            let lifecycle = window.iter().map(|(cost, _)| *cost).sum::<Duration>() / count;
            let read = window.iter().map(|(_, cost)| *cost).sum::<Duration>() / count;
            let bytes = std::fs::metadata(&state).expect("state").len();
            println!(
                "retained={} snapshot_bytes={bytes} lifecycle_4_writes={lifecycle:?} \
                 one_read={read:?}",
                index + 1
            );
        }
    }
}
