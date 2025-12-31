{
  outputs,
  resolveAttrPath,
  attr,
  applyFn,
}:

applyFn (resolveAttrPath attr outputs)
