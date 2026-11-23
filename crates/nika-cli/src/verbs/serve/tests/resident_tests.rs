// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

use super::*;

#[test]
fn http_attach_failure_closes_authority_without_activating_arm() {
    let dir = project("attach-failure", HOURLY_A);
    write_workflow(dir.path(), "doctor.nika");
    let state_root = dir.path().join("serve-state");
    let result = nika_serve::serve_resident_process(
        dir.path(),
        state_root.clone(),
        Some(nika_serve::ServerConfig::new(
            "127.0.0.1:0".parse().expect("address"),
            dir.path().join("workflows"),
            dir.path().join("missing.token"),
        )),
    );
    assert!(
        result.is_err_and(|error| error.contains("token file unreadable")),
        "credential refusal must win"
    );
    assert!(
        !dir.path().join(".nika/arm/doctor/history.ndjson").exists(),
        "ARM remains dormant when listener attachment fails"
    );
    nika_serve::JobStore::open(&state_root).expect("authority lock released after attach failure");
}

/// `--sessions` is an explicit listener option: it needs `--bind`, and only it seats the
/// Session routes.
#[test]
fn sessions_is_an_explicit_listener_option() {
    let command = || <ServeArgs as clap::Args>::augment_args(clap::Command::new("serve"));
    assert!(
        command()
            .try_get_matches_from(["serve", "--sessions"])
            .is_err(),
        "--sessions needs --bind"
    );
    let parse = |argv: &[&str]| {
        let matches = command().try_get_matches_from(argv).expect("flags");
        <ServeArgs as clap::FromArgMatches>::from_arg_matches(&matches).expect("args")
    };
    assert!(parse(&["serve", "--bind", "127.0.0.1:0", "--sessions"]).sessions);
    assert!(!parse(&["serve", "--bind", "127.0.0.1:0"]).sessions);
}

/// C6 · the door and the ceiling are explicit operator options on the
/// listener: `--cost-review` seats the door and never changes a ceiling;
/// `--run-cost-ceiling` takes a non-negative amount (0 is a binding veto,
/// never a disarm) or exactly `none`; both need `--bind`.
#[test]
fn cost_review_and_run_cost_ceiling_are_explicit_listener_options() {
    let command = || <ServeArgs as clap::Args>::augment_args(clap::Command::new("serve"));
    for alone in [
        &["serve", "--cost-review"][..],
        &["serve", "--run-cost-ceiling", "none"][..],
    ] {
        assert!(
            command().try_get_matches_from(alone).is_err(),
            "{alone:?} needs --bind"
        );
    }
    let parse = |extra: &[&str]| {
        let mut argv = vec!["serve", "--bind", "127.0.0.1:0"];
        argv.extend_from_slice(extra);
        let matches = command().try_get_matches_from(argv).expect("flags");
        <ServeArgs as clap::FromArgMatches>::from_arg_matches(&matches).expect("args")
    };
    let args = parse(&["--cost-review", "--run-cost-ceiling", "none"]);
    assert!(args.cost_review);
    assert_eq!(args.run_cost_ceiling.as_deref(), Some("none"));
    let listener = || {
        Some(nika_serve::ServerConfig::new(
            "127.0.0.1:0".parse().expect("address"),
            "workflows",
            "serve.token",
        ))
    };
    let seated = nika_serve::server::seat_cost_review(listener(), true, None);
    assert!(seated.is_ok_and(|config| config.is_some()));
    for refused in ["-1", "-0.5", "NaN", "inf", "one", "", "None"] {
        let outcome = nika_serve::server::seat_cost_review(listener(), true, Some(refused));
        assert_eq!(
            outcome.err(),
            Some(nika_serve::ServerLaunchRefuse::InvalidRunCostCeiling),
            "{refused}"
        );
    }
    for accepted in ["none", "0", "0.5", "2"] {
        assert!(nika_serve::server::seat_cost_review(listener(), false, Some(accepted)).is_ok());
    }
    assert_eq!(
        nika_serve::server::seat_cost_review(None, true, None).err(),
        Some(nika_serve::ServerLaunchRefuse::MissingBindOrWorkflows)
    );
    assert!(matches!(
        nika_serve::server::seat_cost_review(None, false, None),
        Ok(None)
    ));
}

/// Job retention is an explicit option of the persistent resident: it needs
/// no listener (scheduled runs fill the store too), means nothing in a
/// rehearsal, and takes a duration of at least one minute.
#[test]
fn job_retention_is_an_explicit_option_of_the_persistent_resident() {
    let command = || <ServeArgs as clap::Args>::augment_args(clap::Command::new("serve"));
    let parse = |argv: &[&str]| {
        let matches = command().try_get_matches_from(argv).expect("flags");
        <ServeArgs as clap::FromArgMatches>::from_arg_matches(&matches).expect("args")
    };
    let unset = parse(&["serve"]);
    assert!(unset.job_retention.is_none() && unset.job_retention_unsuccessful.is_none());
    let args = parse(&[
        "serve",
        "--job-retention",
        "24h",
        "--job-retention-unsuccessful",
        "7d",
    ]);
    assert_eq!(args.job_retention.as_deref(), Some("24h"));
    assert_eq!(args.job_retention_unsuccessful.as_deref(), Some("7d"));
    for rehearsal in ["--once", "--dry"] {
        for flag in ["--job-retention", "--job-retention-unsuccessful"] {
            assert!(
                command()
                    .try_get_matches_from(["serve", rehearsal, flag, "24h"])
                    .is_err(),
                "{flag} means nothing with {rehearsal}"
            );
        }
    }
    let options = |succeeded, unsuccessful| {
        nika_serve::ResidentOptions::default().with_job_retention(succeeded, unsuccessful)
    };
    assert!(options(Some("24h"), Some("7d")).is_ok());
    assert!(options(None, None).is_ok());
    for refused in ["59s", "0d", "7", "1.5h", "a week", ""] {
        let outcome = options(Some(refused), None);
        assert_eq!(
            outcome.err(),
            Some(nika_serve::ServerLaunchRefuse::InvalidJobRetention),
            "{refused}"
        );
    }
    let message =
        nika_serve::launch_operator_message(nika_serve::ServerLaunchRefuse::InvalidJobRetention);
    assert!(message.contains("--job-retention") && message.contains("one minute"));
}

/// The job limits are explicit options of the persistent resident: each is a
/// whole number of at least one (the grace a duration of at least a second),
/// none needs a listener, and none means anything in a rehearsal.
#[test]
fn job_limits_are_explicit_options_of_the_persistent_resident() {
    const COUNTS: [&str; 5] = [
        "--max-concurrent-jobs",
        "--queue-capacity",
        "--max-jobs",
        "--max-queued-per-owner",
        "--max-concurrent-per-owner",
    ];
    let command = || <ServeArgs as clap::Args>::augment_args(clap::Command::new("serve"));
    let parse = |argv: &[&str]| {
        let matches = command().try_get_matches_from(argv).expect("flags");
        <ServeArgs as clap::FromArgMatches>::from_arg_matches(&matches).expect("args")
    };
    let unset = parse(&["serve"]);
    assert_eq!(
        (
            unset.max_concurrent_jobs,
            unset.queue_capacity,
            unset.max_jobs,
            unset.max_queued_per_owner,
            unset.max_concurrent_per_owner,
            unset.shutdown_grace,
        ),
        (None, None, None, None, None, None)
    );
    let args = parse(&[
        "serve",
        "--max-concurrent-jobs",
        "16",
        "--queue-capacity",
        "5000",
        "--max-jobs",
        "20000",
        "--max-queued-per-owner",
        "50",
        "--max-concurrent-per-owner",
        "2",
        "--shutdown-grace",
        "5m",
    ]);
    assert_eq!(args.max_concurrent_jobs, Some(16));
    assert_eq!(args.queue_capacity, Some(5_000));
    assert_eq!(args.max_jobs, Some(20_000));
    assert_eq!(args.max_queued_per_owner, Some(50));
    assert_eq!(args.max_concurrent_per_owner, Some(2));
    assert_eq!(args.shutdown_grace.as_deref(), Some("5m"));
    for flag in COUNTS {
        for refused in ["-1", "1.5", "many"] {
            assert!(
                command()
                    .try_get_matches_from(["serve", flag, refused])
                    .is_err(),
                "{flag} {refused}"
            );
        }
        for rehearsal in ["--once", "--dry"] {
            assert!(
                command()
                    .try_get_matches_from(["serve", rehearsal, flag, "2"])
                    .is_err(),
                "{flag} means nothing with {rehearsal}"
            );
        }
    }
    assert!(
        command()
            .try_get_matches_from(["serve", "--once", "--shutdown-grace", "5m"])
            .is_err()
    );
    let options = nika_serve::ResidentOptions::default;
    assert!(
        options()
            .with_job_capacity(Some(16), Some(5_000), Some(20_000))
            .and_then(|options| options.with_owner_limits(Some(50), Some(2)))
            .and_then(|options| options.with_shutdown_grace(Some("5m")))
            .is_ok()
    );
    assert_eq!(
        options().with_job_capacity(Some(0), None, None).err(),
        Some(nika_serve::ServerLaunchRefuse::InvalidJobLimit)
    );
    assert_eq!(
        options().with_owner_limits(None, Some(0)).err(),
        Some(nika_serve::ServerLaunchRefuse::InvalidJobLimit)
    );
    assert_eq!(
        options().with_shutdown_grace(Some("0s")).err(),
        Some(nika_serve::ServerLaunchRefuse::InvalidShutdownGrace)
    );
    let message =
        nika_serve::launch_operator_message(nika_serve::ServerLaunchRefuse::InvalidJobLimit);
    for flag in COUNTS {
        assert!(message.contains(flag), "{flag}: {message}");
    }
    let message =
        nika_serve::launch_operator_message(nika_serve::ServerLaunchRefuse::InvalidShutdownGrace);
    assert!(message.contains("--shutdown-grace") && message.contains("one second"));
}

/// What becomes of a job whose run was lost is an option of the persistent
/// resident: a mode word the engine knows and a bound from 1 to 16. A
/// rehearsal has no job to start again.
#[test]
fn a_lost_run_s_policy_is_an_option_of_the_persistent_resident() {
    let command = || <ServeArgs as clap::Args>::augment_args(clap::Command::new("serve"));
    let parse = |argv: &[&str]| {
        let matches = command().try_get_matches_from(argv).expect("flags");
        <ServeArgs as clap::FromArgMatches>::from_arg_matches(&matches).expect("args")
    };
    let unset = parse(&["serve"]);
    assert_eq!(
        (unset.retry_interrupted, unset.retry_max_attempts),
        (None, None)
    );
    let args = parse(&[
        "serve",
        "--retry-interrupted",
        "restart",
        "--retry-max-attempts",
        "5",
    ]);
    assert_eq!(args.retry_interrupted.as_deref(), Some("restart"));
    assert_eq!(args.retry_max_attempts, Some(5));
    for refused in ["-1", "1.5", "many"] {
        assert!(
            command()
                .try_get_matches_from(["serve", "--retry-max-attempts", refused])
                .is_err(),
            "{refused}"
        );
    }
    for rehearsal in ["--once", "--dry"] {
        for (flag, value) in [
            ("--retry-interrupted", "restart"),
            ("--retry-max-attempts", "2"),
        ] {
            assert!(
                command()
                    .try_get_matches_from(["serve", rehearsal, flag, value])
                    .is_err(),
                "{flag} means nothing with {rehearsal}"
            );
        }
    }
    let options = nika_serve::ResidentOptions::default;
    for (mode, bound) in [
        (None, None),
        (Some("off"), Some(1)),
        (Some("restart"), Some(16)),
    ] {
        assert!(
            options().with_retry_interrupted(mode, bound).is_ok(),
            "{mode:?} {bound:?}"
        );
    }
    // `resume` is a mode of a later version: today it is refused, not read
    // as another.
    for word in ["resume", "Restart", ""] {
        assert_eq!(
            options().with_retry_interrupted(Some(word), None).err(),
            Some(nika_serve::ServerLaunchRefuse::InvalidRetryInterrupted),
            "{word:?}"
        );
    }
    for bound in [0, 17] {
        assert_eq!(
            options().with_retry_interrupted(None, Some(bound)).err(),
            Some(nika_serve::ServerLaunchRefuse::InvalidRetryMaxAttempts),
            "{bound}"
        );
    }
    let message = nika_serve::launch_operator_message(
        nika_serve::ServerLaunchRefuse::InvalidRetryMaxAttempts,
    );
    assert!(message.contains("--retry-max-attempts") && message.contains("1 to 16"));
    let message = nika_serve::launch_operator_message(
        nika_serve::ServerLaunchRefuse::InvalidRetryInterrupted,
    );
    assert!(message.contains("--retry-interrupted"), "{message}");
}

/// `--provider-concurrency PROVIDER=N` is a repeatable option of the
/// persistent resident: one cap per provider, a provider the engine knows,
/// and a count of at least one. A rehearsal has no jobs to bound.
#[test]
fn provider_concurrency_is_a_repeatable_option_of_the_persistent_resident() {
    let command = || <ServeArgs as clap::Args>::augment_args(clap::Command::new("serve"));
    let parse = |argv: &[&str]| {
        let matches = command().try_get_matches_from(argv).expect("flags");
        <ServeArgs as clap::FromArgMatches>::from_arg_matches(&matches).expect("args")
    };
    assert!(parse(&["serve"]).provider_concurrency.is_empty());
    let args = parse(&[
        "serve",
        "--provider-concurrency",
        "openai=16",
        "--provider-concurrency",
        "anthropic=8",
    ]);
    assert_eq!(args.provider_concurrency, ["openai=16", "anthropic=8"]);
    for rehearsal in ["--once", "--dry"] {
        assert!(
            command()
                .try_get_matches_from(["serve", rehearsal, "--provider-concurrency", "openai=16"])
                .is_err(),
            "a cap means nothing with {rehearsal}"
        );
    }
    let stated = |values: &[&str]| {
        let values: Vec<String> = values.iter().map(|value| (*value).to_owned()).collect();
        nika_serve::ResidentOptions::default().with_provider_concurrency(&values)
    };
    assert!(stated(&["openai=16", "anthropic=8", "mock=1"]).is_ok());
    assert_eq!(
        stated(&[]).ok(),
        Some(nika_serve::ResidentOptions::default()),
        "no cap changes nothing"
    );
    for refused in [
        &["openai"][..],
        &["openai=0"],
        &["openai=-1"],
        &["openai=016"],
        &["openai=many"],
        &["openai=65536"],
        &["=4"],
        &["nobody=4"],
        &["openai=4", "openai=8"],
    ] {
        assert_eq!(
            stated(refused).err(),
            Some(nika_serve::ServerLaunchRefuse::InvalidProviderConcurrency),
            "{refused:?}"
        );
    }
    let message = nika_serve::launch_operator_message(
        nika_serve::ServerLaunchRefuse::InvalidProviderConcurrency,
    );
    assert!(
        message.contains("--provider-concurrency") && message.contains("PROVIDER=N"),
        "{message}"
    );
}

/// `--cost-policy` is an explicit, repeatable listener option: it needs
/// `--bind`, each occurrence is one rule kept as written, and it never
/// changes the ceiling or seats the review door. The launch reads every rule
/// and refuses one it cannot hold, so a server that starts holds the policy
/// its operator wrote.
#[test]
fn cost_policy_is_an_explicit_repeatable_listener_option() {
    let command = || <ServeArgs as clap::Args>::augment_args(clap::Command::new("serve"));
    let rule = "deepseek/c6-unpriced-fixture:requests=4,output-tokens=64,timeout=30,runs=3/hour";
    assert!(
        command()
            .try_get_matches_from(["serve", "--cost-policy", rule])
            .is_err(),
        "--cost-policy needs --bind"
    );
    let parse = |extra: &[&str]| {
        let mut argv = vec!["serve", "--bind", "127.0.0.1:0"];
        argv.extend_from_slice(extra);
        let matches = command().try_get_matches_from(argv).expect("flags");
        <ServeArgs as clap::FromArgMatches>::from_arg_matches(&matches).expect("args")
    };
    let none = parse(&[]);
    assert!(none.cost_policy.is_empty(), "off unless asked");
    let other = rule.replace("c6-unpriced-fixture", "another-unpriced-fixture");
    let args = parse(&["--cost-policy", rule, "--cost-policy", &other]);
    assert_eq!(args.cost_policy, [rule.to_owned(), other.clone()]);
    assert!(!args.cost_review && args.run_cost_ceiling.is_none());
    let listener = || {
        Some(nika_serve::ServerConfig::new(
            "127.0.0.1:0".parse().expect("address"),
            "workflows",
            "serve.token",
        ))
    };
    let seated = nika_serve::server::seat_cost_policy(listener(), &args.cost_policy);
    assert!(seated.is_ok_and(|config| config.is_some()));
    assert!(matches!(
        nika_serve::server::seat_cost_policy(None, &none.cost_policy),
        Ok(None)
    ));
    let unbounded = ["deepseek/c6-unpriced-fixture:requests=4".to_owned()];
    let Err(refused) = nika_serve::server::seat_cost_policy(listener(), &unbounded) else {
        panic!("a rule that bounds too little was seated");
    };
    let refused = refused.to_string();
    assert!(
        refused.starts_with("standing cost rule ") && refused.contains("is required"),
        "{refused}"
    );
    // The verb refuses the same rule before it reads a project, a credential
    // or a socket, in the words every other launch refusal uses.
    let launch = parse(&[
        "--workflows",
        "workflows",
        "--token-file",
        "serve.token",
        "--cost-policy",
        &unbounded[0],
    ]);
    let out = run(&launch);
    assert_eq!(out.code, exit::WORKFLOW);
    assert_eq!(out.text, format!("serve · {refused}"));
}

/// #1649 · a project that arms nothing says so: `--once` and `--dry` are
/// rehearsals of nothing and exit 0 at once, naming the empty registry and
/// the HTTP door that serves workflows without a beat. Measured on 0.123.2
/// after `nika init --yes`: `--once` exited 0 in silence and `--dry` never
/// returned.
#[test]
fn an_unarmed_project_says_so_and_rehearsals_exit() {
    let dir = project("unarmed", "nika: proj\n");
    let _lease = crate::cwd::enter(dir.path()).expect("enter the unarmed project");
    for (once, dry) in [(true, false), (false, true)] {
        let (sent, received) = std::sync::mpsc::channel();
        // A thread with a deadline: on the broken path `--dry` never returns.
        std::thread::Builder::new()
            .name("unarmed-rehearsal".to_owned())
            .spawn(move || {
                let mut args = serve_args();
                args.once = once;
                args.dry = dry;
                let _ = sent.send(run(&args));
            })
            .expect("spawn the rehearsal");
        let out = received
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("once={once} dry={dry} · a rehearsal of nothing returns"));
        assert_eq!(out.code, exit::OK, "{}", out.text);
        assert!(out.text.contains("nothing armed"), "{}", out.text);
        assert!(out.text.contains("--bind"), "{}", out.text);
        // A fresh project has no `.nika/` yet: the recipe creates it before
        // it writes the token there, or the pasted line fails.
        let mkdir = out.text.find("mkdir -p .nika &&");
        let write = out.text.find("> .nika/serve.token");
        assert!(
            matches!((mkdir, write), (Some(m), Some(w)) if m < w),
            "the recipe creates `.nika/` before writing into it · {}",
            out.text
        );
        assert!(
            !dir.path().join(".nika").exists(),
            "the rehearsal itself creates nothing"
        );
    }
}
