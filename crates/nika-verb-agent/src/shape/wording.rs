// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The words of one schema violation (NIK-402 review): what the repair prompt, the failure
//! record and the trace frame say about a final message the task `schema:` rejected.
//!
//! An object's keys are the model's content as much as its values are. jsonschema's masked
//! wording hides values but quotes keys: a `propertyNames` miss renders the key, and
//! `additionalProperties` / `unevaluatedProperties` list every unexpected key without limit.
//! Those three are worded here with a count and the schema's own words (its declared names,
//! its patterns), and an instance path keeps a segment only where the schema declares that
//! property or the segment is an array index (`*` otherwise). Every violation is cut at
//! [`VIOLATION_KEPT`] bytes.

use jsonschema::ValidationError;
use jsonschema::error::ValidationErrorKind;
use jsonschema::paths::{Location, LocationSegment};
use serde_json::Value;

/// The most bytes one violation keeps, the same text in the repair prompt, the record and the
/// trace frame: a long list of allowed names or a deep path is cut, marked with `…`.
pub(crate) const VIOLATION_KEPT: usize = 256;

/// How deep the walks below follow `$ref` and `allOf` / `anyOf` / `oneOf`: a cyclic or very
/// deep schema stops there and reads as undeclared (`*`).
const DEPTH: usize = 8;

/// One violation before it is listed: `propertyNames` misses at one place with one rule are
/// counted together ([`Word::absorb`]).
pub(crate) struct Word {
    /// The instance path, masked, with its `: ` separator (empty at the root).
    at: String,
    rule: String,
    /// The property names a `propertyNames` miss counts; `None` for any other violation.
    names: Option<usize>,
}

impl Word {
    /// The violation as listed, cut at [`VIOLATION_KEPT`] bytes.
    pub(crate) fn render(&self) -> String {
        let text = match self.names {
            Some(1) => format!(
                "{}1 property name fails propertyNames: {}",
                self.at, self.rule
            ),
            Some(n) => format!(
                "{}{n} property names fail propertyNames: {}",
                self.at, self.rule
            ),
            None => format!("{}{}", self.at, self.rule),
        };
        kept(text)
    }

    /// Count `other` into this word when both are `propertyNames` misses at one place with
    /// one rule; `false` leaves `other` to be listed on its own.
    pub(crate) fn absorb(&mut self, other: &Self) -> bool {
        let (Some(names), Some(more)) = (self.names, other.names) else {
            return false;
        };
        if self.at != other.at || self.rule != other.rule {
            return false;
        }
        self.names = Some(names.saturating_add(more));
        true
    }
}

/// `error` worded against `schema`, the task's whole schema.
pub(crate) fn word(error: &ValidationError<'_>, schema: &Value) -> Word {
    let path = masked_path(error.instance_path(), schema);
    let at = if path.is_empty() {
        String::new()
    } else {
        format!("{path}: ")
    };
    let (rule, names) = match error.kind() {
        ValidationErrorKind::PropertyNames { error: name } => {
            (name.masked_with("the name").to_string(), Some(1))
        }
        ValidationErrorKind::AdditionalProperties { unexpected } => (
            unexpected_keys("additional", unexpected.len(), error, schema, false),
            None,
        ),
        ValidationErrorKind::UnevaluatedProperties { unexpected } => (
            unexpected_keys("unevaluated", unexpected.len(), error, schema, true),
            None,
        ),
        _ => (error.masked().to_string(), None),
    };
    Word { at, rule, names }
}

/// `n` keys the schema does not allow, worded with the names and patterns the schema itself
/// declares where the keyword sits; `through` also reads the subschemas an
/// `unevaluatedProperties` sees (`allOf` / `anyOf` / `oneOf`, a local `$ref`).
fn unexpected_keys(
    which: &str,
    n: usize,
    error: &ValidationError<'_>,
    schema: &Value,
    through: bool,
) -> String {
    let mut allowed = Vec::new();
    if let Some(node) = keyword_parent(schema, error.schema_path()) {
        declared_names(node, schema, through, 0, &mut allowed);
    }
    let noun = if n == 1 { "property" } else { "properties" };
    if allowed.is_empty() {
        return format!("{n} {which} {noun} not allowed");
    }
    format!(
        "{n} {which} {noun} not allowed (the schema allows: {})",
        allowed.join(", ")
    )
}

/// The schema object holding the keyword at `keyword` (its location in the task's schema);
/// `None` when that location is not an object of it (a keyword reached through a `$ref`).
fn keyword_parent<'s>(schema: &'s Value, keyword: &Location) -> Option<&'s Value> {
    let (parent, _) = keyword.as_str().rsplit_once('/')?;
    schema.pointer(parent).filter(|node| node.is_object())
}

/// The property names and patterns `node` declares, quoted, in order and once each.
fn declared_names(node: &Value, root: &Value, through: bool, depth: usize, out: &mut Vec<String>) {
    if depth > DEPTH {
        return;
    }
    let mut push = |name: String| {
        if !out.contains(&name) {
            out.push(name);
        }
    };
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        properties
            .keys()
            .for_each(|name| push(format!("\"{name}\"")));
    }
    if let Some(patterns) = node.get("patternProperties").and_then(Value::as_object) {
        patterns
            .keys()
            .for_each(|p| push(format!("names matching \"{p}\"")));
    }
    if through {
        for sub in applied(node, root) {
            declared_names(sub, root, through, depth + 1, out);
        }
    }
}

/// The subschemas `node` applies in place: its `allOf` / `anyOf` / `oneOf` members and the
/// target of a local `$ref` (`#…`).
fn applied<'s>(node: &'s Value, root: &'s Value) -> Vec<&'s Value> {
    let mut subs: Vec<&Value> = ["allOf", "anyOf", "oneOf"]
        .iter()
        .filter_map(|key| node.get(*key).and_then(Value::as_array))
        .flatten()
        .collect();
    if let Some(target) = (node.get("$ref").and_then(Value::as_str))
        .and_then(|r| r.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer))
    {
        subs.push(target);
    }
    subs
}

/// The subschema `node` declares for property `name`, directly or through the subschemas it
/// applies in place.
fn declared<'s>(node: &'s Value, root: &'s Value, name: &str, depth: usize) -> Option<&'s Value> {
    if depth > DEPTH {
        return None;
    }
    if let Some(child) = node.get("properties").and_then(|p| p.get(name)) {
        return Some(child);
    }
    (applied(node, root).into_iter()).find_map(|sub| declared(sub, root, name, depth + 1))
}

/// The subschema `node` gives array item `index`: its `prefixItems` entry (or the entry of an
/// older draft's `items` array), else its `items`.
fn item<'s>(node: &'s Value, root: &'s Value, index: usize, depth: usize) -> Option<&'s Value> {
    if depth > DEPTH {
        return None;
    }
    let items = node.get("items");
    let own = (node.get("prefixItems").and_then(|p| p.get(index)))
        .or_else(|| {
            items
                .and_then(Value::as_array)
                .and_then(|tuple| tuple.get(index))
        })
        .or_else(|| items.filter(|items| items.is_object()));
    own.or_else(|| (applied(node, root).into_iter()).find_map(|s| item(s, root, index, depth + 1)))
}

/// `path` with every segment the schema does not declare written `*`: an array index is kept,
/// a property is kept only where the schema names it. Once a segment is undeclared, every
/// property below it is too.
fn masked_path(path: &Location, root: &Value) -> String {
    let mut node = Some(root);
    let mut masked = String::new();
    for segment in path {
        masked.push('/');
        match segment {
            LocationSegment::Index(index) => {
                masked.push_str(&index.to_string());
                node = node.and_then(|n| item(n, root, index, 0));
            }
            LocationSegment::Property(name) => {
                node = node.and_then(|n| declared(n, root, &name, 0));
                if node.is_some() {
                    masked.push_str(&name.replace('~', "~0").replace('/', "~1"));
                } else {
                    masked.push('*');
                }
            }
        }
    }
    masked
}

/// `text` cut to [`VIOLATION_KEPT`] bytes on a character boundary, `…` marking the cut.
fn kept(mut text: String) -> String {
    if text.len() <= VIOLATION_KEPT {
        return text;
    }
    let mut end = VIOLATION_KEPT - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('…');
    text
}
