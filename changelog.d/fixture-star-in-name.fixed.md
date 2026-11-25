- **A fixture read through a star inside its name is staged.** A body that
  reads `examples/fixtures/quote-compare-decide/quote-*.txt` now stages the
  directory those files live in, so `nika try quote-compare-decide` takes its
  three quotes along instead of running without them.
