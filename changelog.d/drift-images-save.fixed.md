- **`nika check` no longer advises removing what only an agent's `images: { save: }` uses.** A
  declared input, const or secret read only by the save path was called "declared but never
  referenced", and the `permits.fs.write` entry covering a literal save path was called dead.
  Following either piece of advice broke the file: the save path's reference no longer resolved, or
  the run's write was refused. A templated save path makes no claim about the write grants.
