# Shared helper functions for flake attribute resolution.
#
# These are used by eval.nix and inline expressions in nix.rs.

rec {
  # Check if a nested path exists in an attrset
  hasPath = path: obj:
    let
      attempt = builtins.tryEval (
        if path == [] then true
        else if builtins.isAttrs obj && (obj ? ${builtins.head path})
        then hasPath (builtins.tail path) obj.${builtins.head path}
        else false
      );
    in
      attempt.success && attempt.value;

  # Get a value at a nested path
  getPath = path: obj:
    builtins.foldl' (o: k: o.${k}) obj path;

  # Resolve an attribute path with fallbacks.
  # Mirrors nix's behavior for .#attr references:
  # 1. Try packages.{system}.{attr}
  # 2. Try legacyPackages.{system}.{attr}
  # 3. Try {attr} directly (for top-level outputs)
  #
  # Parameters:
  #   path: dotted string like "hello" or "nixos-branding.nixos-branding-guide"
  #   outputs: the flake outputs attrset
  #
  # Returns the resolved value or throws if not found.
  resolveAttrPath = path: outputs:
    let
      raw = builtins.split "\\." path;
      parts = builtins.filter (x: builtins.isString x && x != "") raw;
      firstPart = builtins.head parts;
      restParts = builtins.tail parts;
      system = builtins.currentSystem;

      # Check if this looks like a per-system category (has system in second position)
      looksLikePerSystem = builtins.length parts >= 2 &&
        builtins.match ".*-.*" (builtins.elemAt parts 1) != null;

      # Known per-system categories
      startsWithKnownCategory = firstPart == "packages" || firstPart == "legacyPackages" ||
        firstPart == "devShells" || firstPart == "apps" || firstPart == "checks" ||
        firstPart == "formatter";

      # Paths to try for unknown first component (not a known category)
      pathsToTry =
        if startsWithKnownCategory && looksLikePerSystem then
          # Already has category and system, just try as-is with fallback
          let
            needsFallback = firstPart == "packages" && !(outputs ? packages) && outputs ? legacyPackages;
            fallbackParts = [ "legacyPackages" ] ++ restParts;
          in
            if needsFallback then [ fallbackParts ] else [ parts ]
        else if startsWithKnownCategory then
          # Has category but no system, insert system
          [ ([ firstPart system ] ++ restParts) ]
        else
          # Unknown first component - try packages, legacyPackages, then direct
          [
            ([ "packages" system ] ++ parts)
            ([ "legacyPackages" system ] ++ parts)
            parts
          ];

      # Find the first valid path
      findFirstValid = paths:
        if paths == [] then null
        else if hasPath (builtins.head paths) outputs
        then builtins.head paths
        else findFirstValid (builtins.tail paths);

      resultPath = findFirstValid pathsToTry;
    in
    if resultPath == null then
      throw "attribute '${path}' not found in flake outputs"
    else
      getPath resultPath outputs;
}
