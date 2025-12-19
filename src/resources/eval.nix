# trix evaluation wrapper
#
# This is the main entry point called by nix-build/nix-shell.
# It reads the flake.nix and flake.lock from the target directory,
# constructs the inputs attrset, and calls the flake's outputs function.

{
  flakeDir, # Path to directory containing flake.nix (as string or path)
  system, # Current system, e.g., "x86_64-linux"
  attr, # Attribute path to select, e.g., "packages.x86_64-linux.default"
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
  # 'self' needs to reference outputs for recursive self-references in flake.nix
  inputs =
    let
      baseInputs = import ./inputs.nix {
        inherit lock flakeDirPath system;
      };
    in
    baseInputs
    // {
      self = baseInputs.self // outputs;
    };

  # Call outputs with inputs (recursive - self references outputs)
  outputs = flake.outputs inputs;

  # Try to get an attribute path, with fallback from packages to legacyPackages
  # This mirrors nix's behavior for .#attr references
  getAttrPathWithFallback =
    path: obj:
    let
      raw = builtins.split "\\." path;
      parts = builtins.filter (x: builtins.isString x && x != "") raw;
      firstPart = builtins.head parts;
      restParts = builtins.tail parts;

      # Check if path starts with "packages" and that category doesn't exist
      needsFallback = firstPart == "packages" && !(obj ? packages) && obj ? legacyPackages;

      # Build fallback path with legacyPackages instead of packages
      fallbackParts = [ "legacyPackages" ] ++ restParts;
    in
    if needsFallback then
      builtins.foldl' (o: k: o.${k}) obj fallbackParts
    else
      builtins.foldl' (o: k: o.${k}) obj parts;

in
getAttrPathWithFallback attr outputs
