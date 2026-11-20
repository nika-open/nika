- **`nika doctor` names the package an npm copy was installed as.** When
  an npm copy shadows the engine on PATH, the remedy used one package name
  for every copy, so after the move to `@nika-open/nika` it would have told
  the owner of an older install to uninstall a package that never put it
  there. It now reads the scope from the copy's path and its resolved
  target: a copy under `node_modules/@supernovae-st/` is removed as
  `@supernovae-st/nika`, any other as `@nika-open/nika`.
