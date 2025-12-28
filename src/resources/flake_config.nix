{ flakePath }:
let
  flake = import (flakePath + "/flake.nix");
  cfg = flake.nixConfig or { };
in
{
  bash-prompt = cfg.bash-prompt or null;
  bash-prompt-prefix = cfg.bash-prompt-prefix or null;
  bash-prompt-suffix = cfg.bash-prompt-suffix or null;
}
