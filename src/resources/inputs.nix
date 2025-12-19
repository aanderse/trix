# Build the 'inputs' attrset from flake.lock
#
# This constructs the inputs that get passed to flake.outputs.
# Uses native flake.lock format (version 7).

{
  lock, # Parsed flake.lock content
  flakeDirPath, # Path to the flake directory
  system, # Current system
}:

let
  nodes = lock.nodes or { };

  # Resolve a follows path through the lock file
  # e.g., ["nixpkgs"] -> root.inputs.nixpkgs -> nixpkgs input
  # e.g., ["foo", "nixpkgs"] -> root.inputs.foo -> foo.inputs.nixpkgs -> final input
  # Includes cycle detection to prevent infinite loops
  resolveFollowsWithVisited =
    visited: path:
    let
      pathKey = builtins.concatStringsSep "/" path;
      newVisited = visited ++ [ pathKey ];
      step =
        nodeName: elem:
        let
          node = nodes.${nodeName};
          ref = node.inputs.${elem} or (throw "trix: input '${elem}' not found in node '${nodeName}'");
        in
        if builtins.isString ref then
          ref
        else
          # Another follows path - check for cycle before recursing
          let
            refKey = builtins.concatStringsSep "/" ref;
          in
          if builtins.elem refKey newVisited then
            throw "trix: circular follows detected: ${
              builtins.concatStringsSep " -> " (newVisited ++ [ refKey ])
            }"
          else
            resolveFollowsWithVisited newVisited ref;
      nodeName = builtins.foldl' step "root" path;
      node = nodes.${nodeName};
    in
    buildInput nodeName node flakeDirPath;

  resolveFollows = resolveFollowsWithVisited [ ];

  # Fetch a source based on the native flake.lock format
  # basePath is used for resolving relative path inputs
  fetchSource =
    name: node: basePath:
    let
      locked = node.locked or { };
      type = locked.type or "unknown";
    in
    if type == "github" then
      builtins.fetchTarball {
        url = "https://github.com/${locked.owner}/${locked.repo}/archive/${locked.rev}.tar.gz";
        sha256 = locked.narHash;
      }
    else if type == "gitlab" then
      let
        host = locked.host or "gitlab.com";
      in
      builtins.fetchTarball {
        url = "https://${host}/${locked.owner}/${locked.repo}/-/archive/${locked.rev}/${locked.repo}-${locked.rev}.tar.gz";
        sha256 = locked.narHash;
      }
    else if type == "sourcehut" then
      let
        host = locked.host or "git.sr.ht";
      in
      builtins.fetchTarball {
        url = "https://${host}/~${locked.owner}/${locked.repo}/archive/${locked.rev}.tar.gz";
        sha256 = locked.narHash;
      }
    else if type == "git" then
      builtins.fetchGit (
        {
          inherit (locked) url;
          inherit (locked) rev;
          inherit (locked) narHash;
        }
        // (if locked ? ref then { inherit (locked) ref; } else { })
      )
    else if type == "path" then
      let
        path = locked.path or node.original.path or "";
      in
      if builtins.substring 0 1 path == "/" then /. + path else basePath + "/${path}"
    else if type == "tarball" then
      builtins.fetchTarball {
        inherit (locked) url;
        sha256 = locked.narHash;
      }
    else if type == "file" then
      builtins.fetchTarball {
        inherit (locked) url;
        sha256 = locked.narHash;
      }
    else if type == "mercurial" || type == "hg" then
      throw "trix: mercurial/hg inputs are not supported (no builtins.fetchMercurial available). Input: '${name}'"
    else
      throw "trix: unknown source type '${type}' for input '${name}'";

  # Build an input value from a fetched source
  # basePath is used for resolving relative path inputs in this node
  buildInput =
    name: node: basePath:
    let
      src = fetchSource name node basePath;
      isFlake = node.flake or true;
    in
    # Non-flake inputs just return the source path
    if !isFlake then
      { outPath = src; }
    # For flake inputs, import their flake.nix and call outputs
    else
      let
        inputFlake = import (src + "/flake.nix");

        # Our lock file may only have follows overrides, not all of the input's deps.
        # Read the input's own flake.lock to get its other dependencies.
        inputFlakeLock =
          if builtins.pathExists (src + "/flake.lock") then
            builtins.fromJSON (builtins.readFile (src + "/flake.lock"))
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
        inputLockNodes = inputFlakeLock.nodes or { };
        inputLockRootInputs = (inputLockNodes.root or { }).inputs or { };

        # Helper to build an input from the transitive flake's own lock
        # Uses src as the base path since we're resolving relative to this input's directory
        buildFromInputLock =
          iname:
          let
            ref = inputLockRootInputs.${iname};
            refNodeName = if builtins.isString ref then ref else builtins.head ref;
            refNode = inputLockNodes.${refNodeName};
          in
          buildInput iname refNode src;

        # What inputs does this flake actually need?
        # Some flakes declare inputs explicitly, others infer them from outputs args
        inputFlakeInputNames =
          if inputFlake ? inputs then
            builtins.attrNames inputFlake.inputs
          else
            builtins.attrNames (builtins.functionArgs inputFlake.outputs);

        # Our overrides from the main lock file (follows, etc.)
        nodeInputs = node.inputs or { };

        # Build each input the flake needs
        inputInputs = builtins.listToAttrs (
          map (iname: {
            name = iname;
            value =
              if nodeInputs ? ${iname} then
                # We have an override (follows or direct ref)
                let
                  ref = nodeInputs.${iname};
                in
                if builtins.isString ref then buildInput iname nodes.${ref} flakeDirPath else resolveFollows ref
              else if inputLockRootInputs ? ${iname} then
                # Use the input's own flake.lock
                buildFromInputLock iname
              else
                # Input not found anywhere - might be optional or error
                throw "trix: cannot find input '${iname}' for '${name}' (not in lock or input's flake.lock)";
          }) inputFlakeInputNames
        );

        # Build self with inputs (flake-parts and similar frameworks need self.inputs)
        # Also need _type = "flake" for flake-parts to recognize this as a flake input
        inputSelf = {
          outPath = src;
          inputs = inputInputs;
          _type = "flake";
        };
        # Use recursive self-reference so flake can reference its own outputs
        inputOutputs = inputFlake.outputs (inputInputs // { self = inputSelf // inputOutputs; });
      in
      inputOutputs
      // {
        outPath = src;
        inputs = inputInputs;
        outputs = inputOutputs;
        _type = "flake";
      };

  # Get input names from root node (locked inputs)
  rootInputs = nodes.root.inputs or { };

  # Build all locked inputs (excluding "root")
  # Handle both string refs ("nixpkgs") and follows refs (["nixpkgs"])
  lockedInputs = builtins.mapAttrs (
    name: ref:
    if builtins.isString ref then
      # Normal reference - build from node
      buildInput name nodes.${ref} flakeDirPath
    else if builtins.isList ref then
      # Root-level follows reference - resolve
      resolveFollows ref
    else
      throw "trix: unexpected reference type for input '${name}'"
  ) rootInputs;

  # Build the 'self' input with inputs (flake-parts needs self.inputs and _type)
  self = {
    outPath = flakeDirPath;
    inputs = lockedInputs;
    _type = "flake";
  };

in
{ inherit self; } // lockedInputs
