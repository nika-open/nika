- **`dom_smoothie`, the readability library behind `extract: article`, comes
  from crates.io again (0.18.2).** Since 0.119.0 the build took it from a fork
  that settled candidate score ties in document order. Upstream released that
  fix in 0.18.1, which the workspace now requires as a minimum, so the engine
  has no git dependency left: `cargo deny` allows no git source, and the Nix
  flake vendors none. One behaviour follows the registry release: on a page
  that reaches the readability stage, an image marked `loading="lazy"` whose
  class does not contain `lazy`, and whose `src` or `srcset` is neither empty
  nor a `data:` URL, keeps that address, even when it is a placeholder file;
  the earlier build copied another attribute that looked like an image
  address (`data-src`, for example) into its `src` or `srcset`. The article
  Markdown of such a page can name a different image URL. An image whose
  class contains `lazy` is treated as before.
