{
  outputs,
  allSystemsFlag,
  showLegacyFlag,
  category,
}:
let
  # Standard per-system output categories (system to name to derivation)
  perSystemAttrs = [
    "packages"
    "devShells"
    "checks"
    "apps"
    "legacyPackages"
  ];

  # Formatter is special: system to derivation (not system to name to derivation)
  formatterAttr = "formatter";

  # Module categories (enumerate names but mark as modules)
  moduleAttrs = [
    "nixosModules"
    "darwinModules"
    "homeManagerModules"
    "flakeModules"
  ];

  # Template categories
  templateAttrs = [ "templates" ];

  # Categories that are known to NOT contain derivations (mark as unknown immediately)
  # This avoids expensive recursive evaluation of things like nixpkgs.lib
  nonDerivationAttrs = [
    "lib"
    "library"
    "htmlDocs"
    "formatterModule"
  ];

  # Get derivation info for current system (extracts name for version display)
  # category parameter is used to determine the output type (devShells vs packages)
  getDerivationInfo =
    category: attrs:
    builtins.listToAttrs (
      map (name: {
        inherit name;
        value =
          let
            drv = attrs.${name};
          in
          if builtins.isAttrs drv && (drv.type or null) == "derivation" then
            {
              _type = "derivation";
              _name = drv.name or null;
              _category = category;
            }
          else if builtins.isAttrs drv && drv ? type && drv.type == "app" then
            {
              _type = "app";
              _program = drv.program or null;
            }
          else
            { _type = "unknown"; };
      }) (builtins.attrNames attrs)
    );

  # Get just names without evaluating (for non-current systems, fast path)
  getNames =
    attrs:
    builtins.listToAttrs (
      map (name: {
        inherit name;
        value = {
          _omitted = true;
        };
      }) (builtins.attrNames attrs)
    );

  # Get derivation names only (filter out non-derivation attrs like callPackage, newScope)
  getDerivationNames =
    attrs:
    let
      names = builtins.attrNames attrs;
      isDerivation =
        name:
        let
          val = attrs.${name};
        in
        builtins.isAttrs val && (val.type or null) == "derivation";
      derivNames = builtins.filter isDerivation names;
    in
    builtins.listToAttrs (
      map (name: {
        inherit name;
        value =
          let
            drv = attrs.${name};
          in
          {
            _type = "derivation";
            _name = drv.name or null;
          };
      }) derivNames
    );

  # Check if an attrset has any derivations (for legacyPackages)
  hasDerivations =
    attrs:
    let
      names = builtins.attrNames attrs;
      isDerivation =
        name:
        let
          val = attrs.${name};
        in
        builtins.isAttrs val && (val.type or null) == "derivation";
    in
    builtins.any isDerivation names;

  # Recursively process an arbitrary nested attrset (for hydraJobs, etc.)
  # Returns nested structure with derivation info at leaves
  # Uses tryEval to handle evaluation errors gracefully
  processNestedAttrs =
    depth: attrs:
    let
      # Try to get attr names, fail gracefully if evaluation errors
      namesResult = builtins.tryEval (builtins.attrNames attrs);
    in
    if !namesResult.success then
      { _unknown = true; }
    else
      builtins.listToAttrs (
        map (
          name:
          let
            # Try to evaluate the value
            valResult = builtins.tryEval attrs.${name};
          in
          {
            inherit name;
            value =
              if !valResult.success then
                { _unknown = true; }
              else
                let
                  val = valResult.value;
                in
                if builtins.isAttrs val then
                  if (val.type or null) == "derivation" then
                    {
                      _type = "derivation";
                      _name = val.name or null;
                    }
                  else if val ? type && val.type == "app" then
                    {
                      _type = "app";
                      _program = val.program or null;
                    }
                  else
                  # Recurse into nested attrset (with depth limit to avoid infinite recursion)
                  if depth < 10 then
                    processNestedAttrs (depth + 1) val
                  else
                    { _unknown = true; }
                else
                  # Non-attrset leaf (function, string, etc.)
                  { _unknown = true; };
          }
        ) namesResult.value
      );

  # Process output category based on its type
  processCategory =
    name: val:
    if builtins.elem name perSystemAttrs && builtins.isAttrs val then
      if name == "legacyPackages" then
        # Special handling for legacyPackages - filter to derivations only
        # Only show if there are actual derivations (not empty)
        # Use tryEval to handle evaluation errors gracefully
        let
          allSystems = builtins.attrNames val;
        in
        builtins.listToAttrs (
          map (sys: {
            name = sys;
            value =
              let
                sysAttrsResult = builtins.tryEval val.${sys};
              in
              if !sysAttrsResult.success then
                { _omitted = true; }
              else
                let
                  sysAttrs = sysAttrsResult.value;
                in
                if !showLegacyFlag then
                  # Mark as legacy omitted - the --legacy flag is what shows these
                  # Use tryEval but default to showing the omit message
                  let
                    hasDerivResult = builtins.tryEval (hasDerivations sysAttrs);
                  in
                  if hasDerivResult.success && !hasDerivResult.value then
                    { } # No derivations, don't show anything
                  else
                    { _legacyOmitted = true; } # Has derivations or check failed
                else if sys == builtins.currentSystem || allSystemsFlag then
                  let
                    derivNamesResult = builtins.tryEval (getDerivationNames sysAttrs);
                  in
                  if derivNamesResult.success then derivNamesResult.value else { _omitted = true; }
                else
                  { _omitted = true; };
          }) allSystems
        )
      else
        # Regular per-system categories (packages, devShells, checks, apps)
        let
          allSystems = builtins.attrNames val;
        in
        builtins.listToAttrs (
          map (sys: {
            name = sys;
            value =
              if sys == builtins.currentSystem || allSystemsFlag then
                getDerivationInfo name val.${sys}
              else
                getNames val.${sys};
          }) allSystems
        )

    else if name == formatterAttr && builtins.isAttrs val then
      let
        allSystems = builtins.attrNames val;
      in
      builtins.listToAttrs (
        map (sys: {
          name = sys;
          value =
            if sys == builtins.currentSystem || allSystemsFlag then
              let
                drv = val.${sys};
              in
              {
                _type = "formatter";
                _name = drv.name or null;
              }
            else
              { _omitted = true; };
        }) allSystems
      )

    else if builtins.elem name moduleAttrs && builtins.isAttrs val then
      builtins.listToAttrs (
        map (n: {
          name = n;
          value = {
            _type = "module";
          };
        }) (builtins.attrNames val)
      )

    else if builtins.elem name templateAttrs && builtins.isAttrs val then
      builtins.listToAttrs (
        map (n: {
          name = n;
          value = {
            _type = "template";
          };
        }) (builtins.attrNames val)
      )

    else if name == "overlays" && builtins.isAttrs val then
      builtins.listToAttrs (
        map (n: {
          name = n;
          value = {
            _type = "overlay";
          };
        }) (builtins.attrNames val)
      )

    else if
      (name == "nixosConfigurations" || name == "darwinConfigurations" || name == "homeConfigurations")
      && builtins.isAttrs val
    then
      builtins.listToAttrs (
        map (n: {
          name = n;
          value = {
            _type = "configuration";
          };
        }) (builtins.attrNames val)
      )

    # For categories known to not contain derivations (lib, htmlDocs, etc.), mark as unknown
    else if builtins.elem name nonDerivationAttrs then
      { _unknown = true; }

    # For any other attrset (hydraJobs, custom outputs, etc.),
    # recursively process to find derivations
    else
      let
        isAttrsResult = builtins.tryEval (builtins.isAttrs val);
      in
      if isAttrsResult.success && isAttrsResult.value then
        processNestedAttrs 0 val
      else
        { _unknown = true; };

in
if outputs ? ${category} then processCategory category outputs.${category} else { }
