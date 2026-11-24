// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The one-off answer a `headless-prompt` hint over an effect teaches: a
//! confirm gate is refused once with `--answer <id>=false`; a choice gate
//! takes one of its `choices:`, since `false` is no answer it accepts.

/// The clause that settles this invocation of the gate `id`, given its
/// `args`: the confirm refusal, or a valid choice (the last literal one as
/// the example, the list named; `<choice>` when the list is templated).
pub(super) fn settle_once(id: &str, args: Option<&serde_json::Value>) -> String {
    let arg = |name: &str| args.and_then(|a| a.get(name));
    if arg("mode").and_then(serde_json::Value::as_str) != Some("choice") {
        return format!("or refuse this invocation with `--answer {id}=false`.");
    }
    let literal: Option<Vec<&str>> = arg("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|list| {
            list.iter()
                .map(|c| c.as_str().filter(|s| !s.contains("${{")))
                .collect()
        });
    match literal.as_deref() {
        Some([.., last]) => {
            let named: Vec<String> = literal.iter().flatten().map(|c| format!("`{c}`")).collect();
            format!(
                "or answer this invocation with one of its `choices:` ({}), e.g. `--answer \
                 {id}={last}`.",
                named.join(", ")
            )
        }
        _ => format!(
            "or answer this invocation with one of its `choices:` (`--answer {id}=<choice>`)."
        ),
    }
}

#[cfg(test)]
mod tests {
    use nika_schema::parser::{ParseMode, parse};
    use nika_schema::source::FileId;

    /// The `headless-prompt` advice for a gate invoked with `args` over a
    /// POST (an effect).
    fn advice(args: &str) -> String {
        let yaml = format!(
            "nika: t\npermits:\n  tools: [\"nika:prompt\", \"nika:fetch\"]\n  net: {{ http: \
             [\"api.example.com\"] }}\ninputs:\n  yes: {{ type: string, default: send }}\ntasks:\n  \
             ask:\n    invoke: {{ tool: \"nika:prompt\", args: {args} }}\n  post:\n    with: {{ \
             pick: \"${{{{ tasks.ask.output }}}}\" }}\n    when: \"${{{{ with.pick != 'no' \
             }}}}\"\n    invoke: {{ tool: \"nika:fetch\", args: {{ url: \
             \"https://api.example.com/hook\", method: POST, body: \"go\" }} }}\n"
        );
        let wf = parse(&yaml, FileId::new(0), ParseMode::Strict).expect("fixture parses");
        let report = crate::check(&wf);
        assert!(report.conformance.is_empty(), "{:?}", report.conformance);
        report
            .hints
            .into_iter()
            .find(|h| h.kind == "headless-prompt" && h.task == "ask")
            .map_or_else(|| panic!("no headless hint for {args}"), |h| h.advice)
    }

    /// A choice gate is answered with one of its choices, never `false`; a
    /// templated list gets a placeholder; a confirm keeps its refusal.
    #[test]
    fn the_one_off_answer_is_one_the_gate_accepts() {
        let listed = advice("{ mode: choice, message: \"send?\", choices: [\"send\", \"no\"] }");
        assert!(
            listed.contains(
                "or answer this invocation with one of its `choices:` (`send`, `no`), e.g. \
                 `--answer ask=no`."
            ),
            "{listed}"
        );
        assert!(!listed.contains("ask=false"), "{listed}");
        let templated = advice(
            "{ mode: choice, message: \"send?\", choices: [\"${{ inputs.yes }}\", \"no\"] }",
        );
        assert!(
            templated.contains("(`--answer ask=<choice>`)") && !templated.contains("ask=false"),
            "{templated}"
        );
        let confirm = advice("{ mode: confirm, message: \"send?\" }");
        assert!(
            confirm.contains("or refuse this invocation with `--answer ask=false`."),
            "{confirm}"
        );
        for kept in [&listed, &templated, &confirm] {
            assert!(
                kept.contains("--answer ask=<value>")
                    && kept.contains("Keep this gate without `default:`"),
                "{kept}"
            );
        }
    }
}
