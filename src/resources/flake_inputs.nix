{ flakePath }:
let
  flake = import (flakePath + "/flake.nix");
  inputs = flake.inputs or { };

  # Convert a value to a URL string, handling paths without store import
  toUrlString =
    v:
    if v == null then
      null
    else if builtins.isPath v then
      builtins.toString v # Use toString to avoid importing path to store
    else
      v;

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
      url = toUrlString (inputAttrs.url or null);
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
