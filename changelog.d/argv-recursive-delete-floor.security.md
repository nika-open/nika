- **The exec floor refuses a recursive delete that leaves the working
  directory, in argv form and on a `shell:` line alike.** The argv floor
  judged a `command:` only by its program identity and an interpreter's
  inline-eval form, so `rm` was an ordinary program whatever its
  arguments. It now refuses `rm` with a recursive flag (`-r`, `-R`,
  `--recursive` or a prefix of it, wherever it sits before `--`) and
  `find` with `-delete`, when a target is absolute or climbs above the
  working directory with `..`, read lexically; a Windows drive prefix
  (`C:\x`) counts as absolute on every host. `nika check` reports a
  literal argv as `NIKA-SEC-001`, and the run refuses the same argv,
  resolved, before spawn. Every simple command of a `shell:` line is
  judged by the same rule, whether it follows `;`, `&`, `|`, a newline or
  a line continuation, or sits behind an assignment, `exec`, `time` or a
  reserved word, with redirection targets set aside. So `rm -fr /x`,
  `rm -r -f /x`, `rm -R ../x` and `true & rm -fr /x` meet the refusal the
  literal `rm -rf /` pattern gave. A delete inside the
  working directory (`["rm", "-rf", "build"]`, `find . -delete`) and a
  non-recursive `rm` stay admitted. The finding's repair names a path
  relative to the working directory (`cwd:` sets it) or `nika:remove_file`
  under a `permits.fs.write` bound. Not judged: symbolic links, and a
  `cwd:` that points outside the project; the OS sandbox built from
  `permits:` still bounds what the child can write.
