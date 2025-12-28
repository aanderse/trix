# trix evaluation wrapper
#
# This is the main entry point called by nix-build/nix-shell.
# It reads the flake.nix and flake.lock from the target directory,
# constructs the inputs attrset, and calls the flake's outputs function.

{
  flakeDir, # Path to directory containing flake.nix (as string or path)
  attr, # Attribute path to select, e.g., "packages.x86_64-linux.default"
  selfInfo ? { }, # Git metadata for self input
}:

let
  # Normalize flakeDir to a path
  flakeDirPath = if builtins.isString flakeDir then /. + flakeDir else flakeDir;

  flakePath = flakeDirPath + "/flake.nix";
  lockPath = flakeDirPath + "/flake.lock";

  # Import shared helpers
  helpers = import ./helpers.nix;

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
  # 'self' needs to reference outputs for recursive self-references in flake.nix
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

  # Call outputs with inputs (recursive - self references outputs)
  outputs = flake.outputs inputs;

in
helpers.resolveAttrPath attr outputs
