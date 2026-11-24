// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The PROMPTS half of the oracle — the kit's portable slash commands, served
//! over MCP so every client gets them, not just the three with a plugin
//! manifest.
//!
//! Why this module exists. MCP defines three server primitives and they
//! differ by WHO drives them: tools are model-controlled, resources are
//! application-controlled, and prompts are USER-controlled — clients
//! surface them as slash commands in a menu the human picks from. Nika
//! shipped tools only, so `/nika:check` and its four siblings existed in
//! Claude Code, Codex and Cursor (three ecosystems, three manifests) and
//! nowhere else. As prompts they reach every wired client from one
//! implementation.
//!
//! The bodies are `include_str!`d from `.agents/plugins/nika/commands/`,
//! the same one-source law `nika-onboard` already follows for the rules,
//! the subagents and the hooks. Those files were the last kit surface the
//! binary never read; now a wording fix in the kit reaches the MCP surface
//! with no second copy to forget.
//!
//! Served: the five audit commands, plus `ideas` (propose workflows from the
//! project) and `capture` (turn the conversation's recent work into a checked
//! workflow). A command whose body calls a plugin helper script (`make`,
//! `run`, `pending`) stays a plugin feature: an MCP client has no plugin root
//! to run it from. `doctor` calls no helper and is simply not served: it
//! diagnoses the client's own wiring, which the client's plugin surface owns.
//!
//! `ideas` and `capture` point at two authoring guides. An MCP-only client
//! has neither the plugin nor a project `nika init` wrote, so both bodies
//! say where the guides live and how to work from the oracle without them.

use serde_json::{Value, json};

macro_rules! command {
    ($file:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../.agents/plugins/nika/commands/",
            $file
        ))
    };
}

/// `(wire name, display title, the kit markdown)`. The description and the
/// argument shape are READ from each file's frontmatter — the kit stays the
/// single source, this table only orders them and names them for display.
const COMMANDS: [(&str, &str, &str); 7] = [
    ("check", "Audit a workflow", command!("check.md")),
    (
        "explain",
        "Explain a workflow or an error code",
        command!("explain.md"),
    ),
    (
        "compile",
        "Compile a reviewable workflow",
        command!("compile.md"),
    ),
    (
        "trace",
        "Read a run's flight recorder",
        command!("trace.md"),
    ),
    (
        "permits",
        "Infer the permits boundary",
        command!("permits.md"),
    ),
    (
        "ideas",
        "Propose workflows for this project",
        command!("ideas.md"),
    ),
    (
        "capture",
        "Turn recent work into a workflow",
        command!("capture.md"),
    ),
];

/// Split a kit command file into `(frontmatter, body)`. A file without the
/// leading fence is all body — the caller then falls back to its display
/// title, so a malformed header degrades instead of vanishing.
fn split_front(md: &str) -> (&str, &str) {
    match md.strip_prefix("---\n") {
        None => ("", md),
        Some(rest) => rest.split_once("\n---\n").unwrap_or(("", md)),
    }
}

/// One `key: value` line out of the frontmatter, unquoted. YAML lets a
/// value carry surrounding quotes (`argument-hint: "[a] [b]"` needs them,
/// because a bare `[` would parse as a flow sequence) — those quotes are
/// syntax, not text, and a client that renders them shows the user a
/// hint with stray punctuation in it.
fn field<'a>(front: &'a str, key: &str) -> Option<&'a str> {
    front.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
            .map(str::trim)
            .map(unquote)
    })
}

/// Strip ONE layer of matching surrounding quotes.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// The kit's bracket convention: `<…>` is required, `[…]` is optional.
fn is_required(hint: &str) -> bool {
    hint.starts_with('<')
}

/// `prompts/list` — name · title · description · argument shape.
///
/// Every command drives off a single `$ARGUMENTS` placeholder (that is the
/// kit's own convention, `new` included), so each prompt declares exactly
/// one argument rather than inventing a per-command parameter list the
/// bodies would not know how to read.
#[must_use]
pub(crate) fn catalog() -> Value {
    let listed: Vec<Value> = COMMANDS
        .iter()
        .map(|(name, title, md)| {
            let (front, _) = split_front(md);
            let hint = field(front, "argument-hint").unwrap_or("");
            let arguments = if hint.is_empty() {
                json!([])
            } else {
                json!([{
                    "name": "arguments",
                    "description": hint,
                    "required": is_required(hint),
                }])
            };
            json!({
                "name": name,
                "title": title,
                "description": field(front, "description").unwrap_or(title),
                "arguments": arguments,
            })
        })
        .collect();
    Value::Array(listed)
}

/// `prompts/get` — the command body as one user-role message, with
/// `$ARGUMENTS` substituted.
///
/// Unlike `tools/call`, `prompts/get` has NO `isError` channel: an unknown
/// name or a missing required argument must come back as a real JSON-RPC
/// error, never as a content block a model would read as instructions.
pub(crate) fn render(name: &str, args: Option<&Value>) -> Result<Value, String> {
    let Some((_, title, md)) = COMMANDS.iter().find(|(n, _, _)| *n == name) else {
        let served: Vec<&str> = COMMANDS.iter().map(|(n, _, _)| *n).collect();
        return Err(format!(
            "unknown prompt `{name}` — nika serves {}",
            served.join(" · ")
        ));
    };
    let (front, body) = split_front(md);
    let hint = field(front, "argument-hint").unwrap_or("");
    let supplied = args
        .and_then(|a| a.get("arguments"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if is_required(hint) && supplied.is_empty() {
        return Err(format!("prompt `{name}` requires an argument: {hint}"));
    }
    Ok(json!({
        "description": field(front, "description").unwrap_or(title),
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": body.trim_start().replace("$ARGUMENTS", supplied) },
        }],
    }))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_portable_kit_commands_are_served_in_order() {
        let catalog = catalog();
        let listed = catalog.as_array().expect("the catalog is an array");
        let names: Vec<&str> = listed.iter().filter_map(|p| p["name"].as_str()).collect();
        assert_eq!(
            names,
            [
                "check", "explain", "compile", "trace", "permits", "ideas", "capture"
            ]
        );
    }

    #[test]
    fn a_served_command_never_needs_the_plugin_root() {
        // An MCP client has no plugin root: a body that calls a helper
        // script under it would hand the model a command it cannot run.
        for (name, _, md) in COMMANDS {
            assert!(
                !md.contains("CLAUDE_PLUGIN_ROOT"),
                "`{name}` calls a plugin helper and cannot be served over MCP"
            );
        }
    }

    #[test]
    fn ideas_and_capture_write_nothing_before_the_person_chooses() {
        // `ideas` proposes and writes nothing until a pick; `capture`
        // checks and never runs. The bodies must keep saying so.
        let ideas = render("ideas", None).expect("ideas renders bare");
        let text = ideas["messages"][0]["content"]["text"]
            .as_str()
            .expect("a text block");
        assert!(text.contains("Write nothing until they pick"), "{text}");
        let capture = render("capture", None).expect("capture renders bare");
        let text = capture["messages"][0]["content"]["text"]
            .as_str()
            .expect("a text block");
        assert!(text.contains("never run"), "{text}");
        assert!(text.contains("do not pass `--answer`"), "{text}");
    }

    #[test]
    fn ideas_and_capture_stand_without_the_plugin_files() {
        // An MCP-only client has no skill folder: a prompt that only says
        // "read the guide" sends it to a file it does not have.
        for name in ["ideas", "capture"] {
            let got = render(name, None).expect("the prompt renders bare");
            let text = got["messages"][0]["content"]["text"]
                .as_str()
                .expect("a text block");
            for needed in [
                ".agents/skills/nika-authoring/references/",
                "nika_examples",
                "Offer `nika init`",
            ] {
                assert!(text.contains(needed), "`{name}` lacks `{needed}`");
            }
        }
    }

    #[test]
    fn every_prompt_carries_the_frontmatter_the_kit_declares() {
        let catalog = catalog();
        for prompt in catalog.as_array().expect("array") {
            let name = prompt["name"].as_str().expect("a name");
            let description = prompt["description"].as_str().unwrap_or("");
            assert!(
                description.len() > 20,
                "{name}: description did not come from the kit frontmatter"
            );
            assert!(
                !prompt["title"].as_str().unwrap_or("").is_empty(),
                "{name}: no display title"
            );
            for argument in prompt["arguments"].as_array().expect("an array") {
                assert!(argument["name"].is_string(), "{name}: argument unnamed");
                assert!(argument["required"].is_boolean(), "{name}: required unset");
            }
        }
    }

    #[test]
    fn a_quoted_frontmatter_hint_reaches_the_client_unquoted() {
        // `compile` carries `argument-hint: "[template] [file.nika]"` —
        // the quotes are YAML syntax (a bare `[` would be a flow sequence),
        // and a client that renders them shows stray punctuation.
        let catalog = catalog();
        let listed = catalog.as_array().expect("array");
        let compile_prompt = listed
            .iter()
            .find(|p| p["name"] == "compile")
            .expect("`compile` is served");
        let hint = compile_prompt["arguments"][0]["description"]
            .as_str()
            .expect("a hint");
        assert!(!hint.starts_with('"'), "the YAML quotes leaked: {hint}");
        assert!(hint.starts_with('['), "the hint lost its bracket: {hint}");
    }

    #[test]
    fn render_substitutes_the_argument_into_the_body() {
        let args = json!({ "arguments": "flow.nika" });
        let got = render("check", Some(&args)).expect("check renders");
        let text = got["messages"][0]["content"]["text"]
            .as_str()
            .expect("a text block");
        assert!(
            text.contains("flow.nika"),
            "the argument never reached the body"
        );
        assert!(
            !text.contains("$ARGUMENTS"),
            "the placeholder survived substitution"
        );
        assert!(
            !text.starts_with("---"),
            "the frontmatter leaked into the message"
        );
    }

    #[test]
    fn a_required_argument_is_refused_when_absent() {
        // `explain` declares `<…>` — required by the kit's own convention.
        let err = render("explain", None).expect_err("a required argument is enforced");
        assert!(err.contains("requires an argument"), "{err}");
    }

    #[test]
    fn an_optional_argument_renders_without_one() {
        // `trace` declares `[…]` — the body has its own no-argument path.
        let got = render("trace", None).expect("trace renders bare");
        assert!(got["messages"][0]["content"]["text"].is_string());
    }

    #[test]
    fn an_unknown_prompt_names_the_whole_surface() {
        let err = render("deploy", None).expect_err("unknown prompts are refused");
        assert!(err.contains("check"), "{err}");
        assert!(err.contains("permits"), "{err}");
    }
}
