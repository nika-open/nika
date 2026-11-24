- **The run guard blocks only inside a Nika project when `nika` is
  missing.** Without the binary on its PATH, the kit's shell hook used to
  refuse every command in every folder. It now refuses them only inside a
  Nika project (a `nika.yaml` or `.nika/` store in the host's project
  directory, the hook's working directory or above). Elsewhere commands
  pass with one `guard_off` warning until the binary is found again, and
  no run is judged there meanwhile. Each refusal now names its own cause:
  a command the judge could not see no longer says to restore the binary.
