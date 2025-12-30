{
  flakeDir,
  isFlake,
  lock,
  selfInfo,
  nixDir,
}:

let
  helpers = import (nixDir + "/helpers.nix");
  inherit (helpers) hasPath getPath resolveAttrPath;

  outputs =
    if isFlake then
      let
        flake = import (flakeDir + "/flake.nix");
        inputs = import (nixDir + "/inputs.nix") {
          inherit lock;
          flakeDirPath = flakeDir;
          inherit selfInfo;
        };
      in
      flake.outputs (inputs // { self = inputs.self // outputs; })
    else
      # Legacy mode: project has default.nix but no flake.nix.
      let
        root = import flakeDir;
      in
      if builtins.isFunction root then root { } else root;
in
{
  inherit
    helpers
    hasPath
    getPath
    resolveAttrPath
    outputs
    ;
}
