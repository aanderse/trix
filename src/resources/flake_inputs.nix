{ flakePath }:
let
  flake = import (flakePath + "/flake.nix");
  inputs = flake.inputs or { };

  # Extract info for a single input
  getInputInfo =
    name:
    let
      input = inputs.${name};
      # Handle both attrset inputs and string shorthand
      inputAttrs = if builtins.isAttrs input then input else { url = input; };
    in
    {
      inherit name;
      url = inputAttrs.url or null;
      follows = inputAttrs.follows or null;
      flake = inputAttrs.flake or true;
      # Get nested input follows (inputs.foo.inputs.bar.follows)
      nestedFollows =
        if inputAttrs ? inputs then
          builtins.listToAttrs (
            builtins.filter (x: x.value != null) (
              map (nestedName: {
                name = nestedName;
                value = inputAttrs.inputs.${nestedName}.follows or null;
              }) (builtins.attrNames inputAttrs.inputs)
            )
          )
        else
          { };
    };

in
map getInputInfo (builtins.attrNames inputs)
