- **The plugin's hooks never take code or a program from the open
  project.** Since 0.100.0 hosts have started the kit's hooks in the
  project they opened, where files there could change what the kit's
  Python imported, and a `PATH` entry naming a folder of the project, or
  since 0.106.0 a relative `NIKA_BIN`, could name the `nika`, `git`, `cat`
  or other program a hook ran. Every hook and helper now keeps only the
  absolute `PATH` entries whose real folder lies outside the project (the
  host's project folder and the working folder, each with its git work
  tree, never the home), makes each Python call in isolated mode, and
  takes `NIKA_BIN` only as an absolute path. The plugin's manifests and
  the wiring `nika init` writes start every hook with `/bin/bash -p`, so
  the host's `PATH` no longer picks the shell and bash reads no
  `BASH_ENV` file and imports no function, and start the Python hooks
  through a launcher that runs them isolated from their first line, so
  `PYTHONHOME` and `PYTHONPATH` no longer reach them. The hooks trust the
  environment the host gives them; a helper a command or an agent starts
  by its own path, and the MCP and language servers' `nika`, still start
  through the host's `PATH`, and a system without `/bin/bash` cannot start
  the hooks at all.
