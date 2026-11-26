// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The host switch: whether this process may start a host program.
//!
//! A registry row names its host by a program name (`codex-acp`, `claude`, `gemini`), and the
//! operating system finds that program on `PATH` when it starts. A test that reaches a presence
//! check, a sign-in check, a probe or a session through the registry would therefore start the
//! real program installed on the developer's machine, under their own sign-in. With
//! [`HOSTS_ENV`] set to `off`, every host program is absent instead: a presence check does not
//! find it, a sign-in check cannot be read (its login command does not start, and the sign-in
//! file it keeps in the person's home is not read), a probe or a session refuses as for a
//! missing binary, and no process starts.
//!
//! The repository's `.cargo/config.toml` sets `NIKA_HARNESS_HOSTS = "off"` for every process
//! Cargo starts (`cargo test`, `cargo nextest run`, `cargo run`). Cargo's `[env]` never replaces
//! a variable that is already set, so `NIKA_HARNESS_HOSTS=on cargo run -p nika-cli -- ...`
//! starts hosts again, and so does any value other than `off`. The switch is read when a host
//! would start, never at build time: a release build or an installed `nika` reads no Cargo
//! configuration and starts hosts unless the operator exports `NIKA_HARNESS_HOSTS=off` itself.
//!
//! A host program is known by its file name, whatever directory names it: the product and ACP
//! programs of the shipped runtimes ([`nika_types::access::HarnessRuntime::ALL`]), with a
//! Windows `.exe` or `.cmd` suffix read as the same name. A test that starts its own stub under a
//! host's name says so in code (the test constructors that name an explicit stub binary), never
//! through the variable.

use std::ffi::OsStr;
use std::path::Path;

/// The variable that keeps every host program absent when it reads `off`.
pub const HOSTS_ENV: &str = "NIKA_HARNESS_HOSTS";

/// Whether [`HOSTS_ENV`] keeps every host program absent in this process: a surface that
/// lists the seats says they were not looked for, never that they are not installed.
#[must_use]
pub fn hosts_switched_off() -> bool {
    Hosts::from_env() == Hosts::Off
}

/// Whether host programs may start in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hosts {
    /// A host program starts like any other program.
    Start,
    /// Every host program is absent: nothing named like one is found or started.
    Off,
}

impl Hosts {
    /// The switch as this process's environment sets it.
    pub(crate) fn from_env() -> Self {
        #[allow(clippy::disallowed_methods)] // the switch's own boundary, as DISABLE_ENV's
        let value = std::env::var_os(HOSTS_ENV);
        Self::from_value(value.as_deref())
    }

    /// `off`, in any ASCII case, keeps hosts absent; any other value, or none, starts them.
    pub(crate) fn from_value(value: Option<&OsStr>) -> Self {
        match value {
            Some(value) if value.eq_ignore_ascii_case("off") => Self::Off,
            _ => Self::Start,
        }
    }

    /// Whether `program` may be looked for or started. The refusal says which program is
    /// absent and why, in the words a missing binary's refusal carries.
    ///
    /// # Errors
    /// The switch is off and `program` names a host program.
    pub(crate) fn admit(self, program: &Path) -> Result<(), String> {
        if self == Self::Off && is_host(program) {
            return Err(format!(
                "`{}` is not started: {HOSTS_ENV}=off keeps every host program absent \
                 (set {HOSTS_ENV}=on to start it)",
                program.display()
            ));
        }
        Ok(())
    }
}

/// Whether `program` names a host program of a shipped runtime, by its file name.
fn is_host(program: &Path) -> bool {
    let windows_launcher = program.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("cmd")
    });
    let name = if windows_launcher {
        program.file_stem()
    } else {
        program.file_name()
    };
    let Some(name) = name else {
        return false;
    };
    nika_types::access::HarnessRuntime::ALL
        .iter()
        .any(|runtime| name == runtime.detect_bin || name == runtime.acp_bin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{AuthProbe, registry_with};

    #[test]
    fn only_off_turns_the_hosts_off() {
        for off in ["off", "OFF", "Off"] {
            assert_eq!(
                Hosts::from_value(Some(OsStr::new(off))),
                Hosts::Off,
                "{off}"
            );
        }
        for start in ["on", "", "0", "false", "offline"] {
            assert_eq!(
                Hosts::from_value(Some(OsStr::new(start))),
                Hosts::Start,
                "{start}"
            );
        }
        assert_eq!(Hosts::from_value(None), Hosts::Start);
    }

    /// Every program a shipped row starts is a host: its ACP command and the sign-in command
    /// its auth surface runs. A new row cannot escape the switch by its name.
    #[test]
    fn every_program_a_shipped_row_starts_is_a_host() {
        let rows = registry_with(&|_| None).expect("the shipped table loads");
        assert_eq!(rows.len(), 8);
        for row in &rows {
            let command = Path::new(&row.adapter.command);
            assert!(
                is_host(command),
                "{}: `{}`",
                row.adapter.id,
                command.display()
            );
            if let AuthProbe::Command { command, .. } = row.auth {
                assert!(
                    is_host(Path::new(command)),
                    "{}: `{command}`",
                    row.adapter.id
                );
            }
        }
        for runtime in nika_types::access::HarnessRuntime::ALL {
            assert!(is_host(Path::new(runtime.detect_bin)), "{}", runtime.id);
        }
    }

    /// A host is known by its file name whatever directory holds it; a program that is not a
    /// host, or a host's bundled script, is admitted when the switch is off.
    #[test]
    fn the_switch_refuses_a_host_by_its_file_name_only() {
        for host in [
            "codex",
            "codex-acp",
            "/opt/homebrew/bin/claude",
            "./bin/claude-agent-acp",
            "C:/tools/codex.cmd",
            "gemini.EXE",
        ] {
            let refused = Hosts::Off.admit(Path::new(host)).expect_err(host);
            assert_eq!(
                refused,
                format!(
                    "`{host}` is not started: NIKA_HARNESS_HOSTS=off keeps every host program \
                     absent (set NIKA_HARNESS_HOSTS=on to start it)"
                )
            );
            assert_eq!(Hosts::Start.admit(Path::new(host)), Ok(()), "{host}");
        }
        for other in [
            "python3",
            "true",
            "/bin/sh",
            "/usr/bin/env",
            "node_modules/@openai/codex/bin/codex.js",
            "codex-acp/dist/index.js",
            "codexx",
            "",
        ] {
            assert_eq!(Hosts::Off.admit(Path::new(other)), Ok(()), "{other}");
        }
    }

    /// The repository's `.cargo/config.toml` hands the switch to every test Cargo starts. A
    /// developer may set it `on` for a run of their own; a test binary started outside Cargo
    /// does not get it and fails here, saying so.
    #[test]
    fn the_cargo_configuration_hands_the_switch_to_tests() {
        #[allow(clippy::disallowed_methods)] // the switch's own boundary, read by its test
        let value = std::env::var_os(HOSTS_ENV);
        assert!(
            value.is_some(),
            "{HOSTS_ENV} is unset: run the tests through cargo, whose `.cargo/config.toml` sets \
             it to `off`, so that no test starts a real host program"
        );
    }
}
