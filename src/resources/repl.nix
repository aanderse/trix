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
}:

let
  # Normalize flakeDir to a path
  flakeDirPath = if builtins.isString flakeDir then /. + flakeDir else flakeDir;

  flakePath = flakeDirPath + "/flake.nix";
  lockPath = flakeDirPath + "/flake.lock";

  # Read and parse the lock file
  lockExists = builtins.pathExists lockPath;
  lock =
    if lockExists then
      builtins.fromJSON (builtins.readFile lockPath)
    else
      {
        nodes = {
          root = {
            inputs = { };
          };
        };
        root = "root";
        version = 7;
      };

  # Import the flake
  flake = import flakePath;

  # Build inputs from lock file
  inputs =
    let
      baseInputs = import ./inputs.nix {
        inherit lock flakeDirPath selfInfo;
      };
    in
    baseInputs
    // {
      self = baseInputs.self // outputs;
    };

  # Call outputs with inputs
  outputs = flake.outputs inputs;

  # Build self (the flake with outputs merged)
  inherit (inputs) self;

  # Convenience: expose nixpkgs as 'pkgs' if available
  pkgs =
    if
      inputs ? nixpkgs && inputs.nixpkgs ? legacyPackages && inputs.nixpkgs.legacyPackages ? ${builtins.currentSystem}
    then
      inputs.nixpkgs.legacyPackages.${builtins.currentSystem}
    else
      null;

  # Convenience: expose lib if nixpkgs is available
  lib = if inputs ? nixpkgs && inputs.nixpkgs ? lib then inputs.nixpkgs.lib else null;

  # Build the result set, only including non-null values
  # Include outputs at top level to match nix repl behavior
  result =
    outputs
    // {
      inherit self inputs outputs;
    }
    // (if pkgs != null then { inherit pkgs; } else { })
    // (if lib != null then { inherit lib; } else { });

in
result
