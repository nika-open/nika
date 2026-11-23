// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! What a declaration states about the jobs it starts: an owner and labels.
//!
//! Both are opaque to the engine. An application writes them so it can find
//! its jobs again; they name nobody the engine knows and grant nothing.
//!
//! This module is the one law of their shape. A schedule declaration is
//! judged by it here, and a resident judges the jobs it admits by the same
//! functions, so what a schedule carries is always something a job may carry.

use std::collections::BTreeMap;

/// Longest stated owner, in bytes.
pub const MAX_OWNER_BYTES: usize = 128;
/// Most labels one declaration or job carries.
pub const MAX_LABELS: usize = 16;
/// Longest label key, in bytes.
pub const MAX_LABEL_KEY_BYTES: usize = 64;
/// Longest label value, in bytes.
pub const MAX_LABEL_VALUE_BYTES: usize = 256;

/// 1 to 128 bytes of `[A-Za-z0-9._:@-]`.
#[must_use]
pub fn valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= MAX_OWNER_BYTES
        && owner.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'@' | b'-')
        })
}

/// 1 to 64 bytes of `[A-Za-z0-9._/-]`: no colon, so a `key:value` filter
/// splits on its first colon.
#[must_use]
pub fn valid_label_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_LABEL_KEY_BYTES
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
}

/// At most 256 bytes without a control character.
#[must_use]
pub fn valid_label_value(value: &str) -> bool {
    value.len() <= MAX_LABEL_VALUE_BYTES && !value.chars().any(char::is_control)
}

/// At most 16 labels, every key and value inside its bounds.
#[must_use]
pub fn valid_labels(labels: &BTreeMap<String, String>) -> bool {
    labels.len() <= MAX_LABELS
        && labels
            .iter()
            .all(|(key, value)| valid_label_key(key) && valid_label_value(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_is_one_to_128_bytes_of_its_alphabet() {
        for owner in ["u", "user_123", "org:acme@eu-west.1", &"a".repeat(128)] {
            assert!(valid_owner(owner), "{owner} is a legal owner");
        }
        for owner in ["", "user 123", "user/123", "üser", "a\n", &"a".repeat(129)] {
            assert!(!valid_owner(owner), "{owner:?} is refused");
        }
    }

    #[test]
    fn a_label_key_never_carries_the_filter_separator() {
        for key in ["app", "billing/plan", "tier.v2", "a-b_c", &"k".repeat(64)] {
            assert!(valid_label_key(key), "{key} is a legal key");
        }
        for key in ["", "app:eu", "a b", "clé", &"k".repeat(65)] {
            assert!(!valid_label_key(key), "{key:?} is refused");
        }
    }

    #[test]
    fn a_label_value_is_bounded_text_without_control_characters() {
        for value in ["", "pro", "été 2026: bêta", &"v".repeat(256)] {
            assert!(valid_label_value(value), "{value} is a legal value");
        }
        for value in ["line\nbreak", "tab\there", "nul\0", &"v".repeat(257)] {
            assert!(!valid_label_value(value), "{value:?} is refused");
        }
    }

    #[test]
    fn labels_are_bounded_in_number_and_in_shape() {
        let sized = |count: usize| {
            (0..count)
                .map(|index| (format!("k{index}"), String::new()))
                .collect::<BTreeMap<_, _>>()
        };
        assert!(valid_labels(&BTreeMap::new()));
        assert!(valid_labels(&sized(16)));
        assert!(!valid_labels(&sized(17)));
        let colon = BTreeMap::from([("app:eu".to_owned(), "x".to_owned())]);
        assert!(!valid_labels(&colon));
        let control = BTreeMap::from([("app".to_owned(), "a\nb".to_owned())]);
        assert!(!valid_labels(&control));
    }
}
