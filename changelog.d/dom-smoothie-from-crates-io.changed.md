- **`dom_smoothie`, the readability library behind `extract: article`, comes
  from crates.io again (0.18.2).** Since 0.119.0 the build took it from a fork
  that settled candidate score ties in document order. Upstream released that
  fix in 0.18.1, which the workspace now requires as a minimum, so the engine
  has no git dependency left: `cargo deny` allows no git source, and the Nix
  flake vendors none. One behaviour follows the registry release: on a page
  that reaches the readability stage, an image marked `loading="lazy"` that
  already has a working `src` keeps it, where the earlier build replaced it
  with another attribute that looked like an image address (`data-src`, for
  example). The article Markdown of such a page can name a different image
  URL; the rule stage already let a working `src` win.
