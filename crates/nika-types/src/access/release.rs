// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The adapter releases a measured row was made on, as data the client compares with the
//! release an adapter reports on `initialize` (`agentInfo.version`). A row's display string
//! (`measured_on`) is for a person; this is what the code compares.

use alloc::string::String;
use core::fmt;

/// One release of a harness adapter: `MAJOR.MINOR.PATCH`, as the adapter reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct AdapterRelease {
    /// The major number.
    pub major: u32,
    /// The minor number.
    pub minor: u32,
    /// The patch number.
    pub patch: u32,
}

impl AdapterRelease {
    /// The release `major.minor.patch`.
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The release an adapter reported, read strictly: three decimal numbers separated by
    /// dots, with no prefix, suffix, whitespace or leading zero. Anything else (`v1.13.1`,
    /// `1.13`, `1.13.1-beta.1`) is a release this client cannot compare, never a measured one.
    #[must_use]
    pub fn parse(reported: &str) -> Option<Self> {
        let mut parts = reported.split('.');
        let major = number(parts.next()?)?;
        let minor = number(parts.next()?)?;
        let patch = number(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self::new(major, minor, patch))
    }

    /// Whether the release an adapter reported is one of `measured`. A report that does not
    /// parse is never measured.
    #[must_use]
    pub fn measured_among(reported: &str, measured: &[Self]) -> bool {
        Self::parse(reported).is_some_and(|release| measured.contains(&release))
    }
}

/// One decimal component: digits only, no sign and no leading zero.
fn number(part: &str) -> Option<u32> {
    let leading_zero = part.len() > 1 && part.starts_with('0');
    if part.is_empty() || leading_zero || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

impl fmt::Display for AdapterRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The release an adapter reported, when it reported one: an empty or blank text is none.
pub(super) fn reported_release(reported: Option<&str>) -> Option<&str> {
    reported.filter(|release| !release.trim().is_empty())
}

/// The release an adapter reported on `initialize` (`agentInfo.version`), as a sentence that
/// names it repeats it. `None` when it reported none, or an empty or blank text. Otherwise its
/// text in printable ASCII with no backtick (any other character, and a backtick, which would
/// close the code span it is printed in, reads `?`), cut at 40 characters, the cut marked by
/// `…`. The note of a carried session (`HarnessBoundary::carried_note`) and the service tier's
/// refusal of an unmeasured release both print a reported release this way.
#[must_use]
pub fn shown_release(reported: Option<&str>) -> Option<String> {
    reported_release(reported).map(shown)
}

/// The most characters of an adapter's own release text a sentence repeats.
const SHOWN_CHARS: usize = 40;

/// An adapter's reported release as a sentence repeats it: printable ASCII only (any other
/// character, and a backtick, which would close the code span the sentence prints it in,
/// reads `?`), at most [`SHOWN_CHARS`] characters, then `…`.
fn shown(reported: &str) -> String {
    let mut out: String = reported
        .chars()
        .take(SHOWN_CHARS)
        .map(|c| {
            if (c.is_ascii_graphic() || c == ' ') && c != '`' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if reported.chars().count() > SHOWN_CHARS {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn a_release_is_read_strictly_and_printed_as_read() {
        assert_eq!(
            AdapterRelease::parse("0.81.1"),
            Some(AdapterRelease::new(0, 81, 1))
        );
        assert_eq!(
            AdapterRelease::parse("1.13.1"),
            Some(AdapterRelease::new(1, 13, 1))
        );
        assert_eq!(
            AdapterRelease::parse("0.112.0"),
            Some(AdapterRelease::new(0, 112, 0))
        );
        assert_eq!(AdapterRelease::new(0, 112, 0).to_string(), "0.112.0");
        for unreadable in [
            "",
            "1",
            "1.13",
            "1.13.1.0",
            "v1.13.1",
            " 1.13.1",
            "1.13.1 ",
            "1.13.1-beta.1",
            "1.13.1+build",
            "01.13.1",
            "1.013.1",
            "1.13.01",
            "1..1",
            "-1.13.1",
            "+1.13.1",
            "1.13.x",
            "nightly",
            "1.13.99999999999",
        ] {
            assert_eq!(AdapterRelease::parse(unreadable), None, "{unreadable:?}");
        }
        assert_eq!(
            AdapterRelease::parse("0.0.0"),
            Some(AdapterRelease::new(0, 0, 0))
        );
    }

    #[test]
    fn only_a_listed_release_is_measured() {
        let measured = [AdapterRelease::new(1, 13, 1)];
        assert!(AdapterRelease::measured_among("1.13.1", &measured));
        for other in [
            "1.13.0", "1.13.10", "1.13", "1.14.0", "2.12.0", "v1.13.1", "",
        ] {
            assert!(
                !AdapterRelease::measured_among(other, &measured),
                "{other:?}"
            );
        }
        assert!(!AdapterRelease::measured_among("1.13.1", &[]));
    }

    #[test]
    fn a_reported_release_is_repeated_printable_and_bounded() {
        assert_eq!(shown("0.112.0"), "0.112.0");
        assert_eq!(shown("1.0\u{1b}[31m\n"), "1.0?[31m?");
        assert_eq!(
            shown("1.0` x `y"),
            "1.0? x ?y",
            "a backtick never closes a code span"
        );
        let long = "9".repeat(50);
        let repeated = shown(&long);
        assert_eq!(repeated.chars().count(), SHOWN_CHARS + 1);
        assert!(repeated.ends_with('…'), "{repeated}");
    }

    /// What a sentence prints for a reported release: nothing for none or a blank text, the
    /// filtered and bounded text otherwise.
    #[test]
    fn a_shown_release_is_none_when_blank_and_filtered_otherwise() {
        for blank in [None, Some(""), Some(" "), Some("\t \n")] {
            assert_eq!(shown_release(blank), None, "{blank:?}");
        }
        assert_eq!(shown_release(Some("1.13.1")).as_deref(), Some("1.13.1"));
        assert_eq!(shown_release(Some("1.0` x")).as_deref(), Some("1.0? x"));
        let long = "9".repeat(50);
        assert_eq!(shown_release(Some(&long)), Some(shown(&long)));
    }
}
