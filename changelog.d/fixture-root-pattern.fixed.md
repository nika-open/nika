- **An example that names its fixtures through a star at the root of `examples/fixtures/` gets
  them.** `nika try`, `nika run` on an example and `nika init --example` stage the fixture files an
  example names. A name such as `examples/fixtures/*.csv`, `examples/fixtures/top-*.csv` or
  `examples/fixtures/**` staged nothing, because the star sat in the first segment. That segment is
  now a pattern over the fixture root, and every file or folder it matches is staged; `*` or `**`
  alone stages the whole shelf. The `11-lift-the-door` lesson bounds its read to
  `examples/fixtures/*.csv` and lets the caller pick the file: it now gets the five root `.csv`
  files its bound admits, where it got only the two its text spells out.
