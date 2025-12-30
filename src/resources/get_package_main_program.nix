{
  outputs,
  resolveAttrPath,
  attr,
}:

let
  pkg = resolveAttrPath attr outputs;
  # Get mainProgram from meta, or fall back to pname/name
  mainProgram = pkg.meta.mainProgram or null;
  pname = pkg.pname or null;
  # Strip version from name (e.g., "hello-2.10" -> "hello")
  name = pkg.name or null;
  nameWithoutVersion =
    if name == null then
      null
    else
      let
        parts = builtins.match "(.+)-[0-9].*" name;
      in
      if parts == null then name else builtins.head parts;
in
if mainProgram != null then
  mainProgram
else if pname != null then
  pname
else if nameWithoutVersion != null then
  nameWithoutVersion
else
  null
