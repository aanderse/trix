# trix REPL wrapper
#
# Provides an interactive environment for exploring flakes.
# Usage: nix repl --file repl.nix --arg flakeDir /path/to/flake --arg selfInfo '{}'
#
# Exposes:
#   self     - the flake with outputs
#   inputs   - all flake inputs
#   outputs  - full flake outputs
#   pkgs     - nixpkgs (if available as input)
#   lib      - nixpkgs.lib (if available)
#   packages, devShells, etc. - all flake outputs at top level (like nix repl)

{
  flakeDir, # Path to directory containing flake.nix (as string or path)
  selfInfo ? { }, # Git metadata for self input (rev, shortRev, etc.)
  isFlake ? true, # Whether to treat this as a flake
  lock ? { }, # Flake lock file content
}:

let
  # Normalize flakeDir to a path
  flakeDirPath = if builtins.isString flakeDir then /. + flakeDir else flakeDir;

  # Import helpers for consistency if needed, but we keep this standalone mostly using common inputs logic.

  result =
    if isFlake then
      let
        flakePath = flakeDirPath + "/flake.nix";
        flake = import flakePath;

        # Build inputs using shared inputs.nix
        baseInputs = import ./inputs.nix {
          inherit lock flakeDirPath selfInfo;
        };

        inputs = baseInputs // {
          self = baseInputs.self // outputs;
        };

        outputs = flake.outputs inputs;
        inherit (inputs) self;

        pkgs =
          if
            inputs ? nixpkgs
            && inputs.nixpkgs ? legacyPackages
            && inputs.nixpkgs.legacyPackages ? ${builtins.currentSystem}
          then
            inputs.nixpkgs.legacyPackages.${builtins.currentSystem}
          else
            null;

        lib = if inputs ? nixpkgs && inputs.nixpkgs ? lib then inputs.nixpkgs.lib else null;
      in
      outputs
      // {
        inherit self inputs outputs;
      }
      // (if pkgs != null then { inherit pkgs; } else { })
      // (if lib != null then { inherit lib; } else { })
    else
      # Legacy mode
      let
        root = import flakeDirPath;
        outputs = if builtins.isFunction root then root { } else root;
      in
      # For legacy, we just return outputs and maybe empty inputs/self to avoid errors if user expects them
      outputs
      // {
        inherit outputs;
        inputs = { };
        self = outputs; # Approximate self as outputs
      };

in
result
