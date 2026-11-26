- **`dom_smoothie`, the readability library behind `extract: article`, comes
  from crates.io again (0.18.2).** Since 0.119.0 the build took it from a fork
  that settled candidate score ties in document order. Upstream released that
  fix in 0.18.1, which the workspace now requires as a minimum, so the engine
  has no git dependency left: `cargo deny` allows no git source, and the Nix
  flake vendors none. Article extraction is unchanged.
