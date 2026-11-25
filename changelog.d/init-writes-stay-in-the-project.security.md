- **`nika init` writes only inside the project and never through a link.**
  Since 0.90.0 `init` wrote each file by its path, so a link in the
  project, or a linked folder such as `.claude`, could carry a write to a
  file outside it, the user's global editor settings included, and
  `--force` overwrote what the link reached. Where `init` must write, it
  now refuses a link at the file's name or a folder that leads outside
  the project, with one report row and exit 3, `--force` included, even
  when the way comes back into the project: each link is read one step
  at a time, and a step outside ends it. It writes into the real folder
  it checked, without resolving the path again. That write still reaches
  the checked folder by its path, so a process already running inside the
  project could race it within a very short window. Where
  it would only leave an existing file as it is, a link there is kept and
  never followed, with a row and exit 0. A link to a file inside the
  project is kept under `--force` too, as with `CLAUDE.md` pointing at
  `AGENTS.md`. A `.gitignore` that is a link is neither read nor
  written, and its row says to replace it with a regular file holding
  the entries: git does not read a `.gitignore` that is a link, whatever
  the file it points at holds, where `init` used to add them there.
  `nika compile`, which adds the same entry beside a workflow it writes,
  now refuses with exit 3, before it writes, when that `.gitignore` is a
  link, for the same reason, or not a regular file, where it used to
  append through the link.
