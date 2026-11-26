// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! NIK-385 · the child traces a journal cites (spec 14 law 8) that are not
//! beside it. The citations parse through the store's ONE reader
//! ([`nika_dap::store::cited_children`]), the one retention keeps runs by,
//! so the two cannot disagree on what a run cites. A missing child is said
//! on its own line: never a `FINDING`, never the tier, never the exit.

use std::path::Path;

/// One line per child trace `raw` cites that is not a file in the directory
/// of `trace`, in citation order. A journal the tolerant reader refuses
/// cites nothing here (the walk already judged it).
pub(super) fn missing_child_lines(trace: &str, raw: &str) -> Vec<String> {
    let Ok(recovered) = nika_dap::recover::recover_events(raw, trace) else {
        return Vec::new();
    };
    let dir = Path::new(trace).parent().unwrap_or_else(|| Path::new(""));
    nika_dap::store::cited_children(&recovered.events)
        .into_iter()
        .filter(|child| !dir.join(&child.trace_id).is_file())
        .map(|child| {
            let item = child
                .index
                .map_or_else(String::new, |index| format!(" item {index}"));
            format!(
                "missing child trace `{}` · task `{}`{item} — this run's evidence is incomplete",
                super::sanitize(&child.trace_id),
                super::sanitize(&child.task),
            )
        })
        .collect()
}
