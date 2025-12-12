# trix

> `trix` - trick yourself into flakes

`trix` provides a flake-like experience using "legacy" nix commands (`nix-build`, `nix-shell`, `nix-instantiate`) under the hood. `trix` operates on `flake.nix` files and produces native `flake.lock` files, but doesn't require the experimental flakes feature to be enabled.

```console
aaron@framework ~> trix --verbose build ~/code/trix#default
+ nix-build /nix/store/68ay9lswdj3r3bn01pjy966kwpax2l80-trix-0.1.0/share/trix/nix/eval.nix --arg flakeDir /home/aaron/code/trix --argstr system x86_64-linux --argstr attr packages.x86_64-linux.default -o result
/nix/store/68ay9lswdj3r3bn01pjy966kwpax2l80-trix-0.1.0
```


## why?

> While `flake.nix` has reached critical mass and there is no going back at this point, this doesn't mean we are stuck with the current implementation of Flakes.


## comparison

### vs `nix flake`

- `nix flake` copies your entire flake to the store before evaluation; `trix` only copies what derivation `src` attributes reference
- `nix flake` enforces a certain degree of purity; `trix` allows traditional impure evaluation
- `nix flake` requires experimental features; `trix` uses stable nix commands, for the most part
- `nix flake` supports the full flake specification; `trix` supports a practical subset

### vs plain `nix-build`/`nix-shell`

- plain nix commands don't understand `flake.nix` or `flake.lock`
- `trix` brings structured inputs and reproducible locking to legacy commands
- `trix` provides a familiar CLI for those used to `nix flake` commands


## Implementation

`trix` is a small `python` tool, _written entirely by [claude](https://claude.ai/)_, that:

1. Parses `flake.nix` to extract inputs (using `nix-instantiate`)
2. Locks inputs using `nix flake prefetch` (the one flakes command we use)
3. Generates a native `flake.lock` file
4. Evaluates outputs by constructing the inputs attrset and calling the flake's `outputs` function
5. Builds/shells using `nix-build`/`nix-shell` with the constructed inputs

The lock file is fully compatible with `nix flake` - you can use `trix` and `nix flake` interchangeably on the same project.


### direnv integration

`trix` includes a direnv library for automatic environment activation. Add to your `~/.config/direnv/direnvrc`:

```bash
# If installed via nix profile:
source ~/.nix-profile/share/trix/direnvrc
# Or with a direct store path:
source /nix/store/...-trix-0.1.0/share/trix/direnvrc
```

Then in your project's `.envrc`:

```bash
use trix
# or for a specific devShell:
use trix .#myshell
```


## See also

- [nix flakes](https://wiki.nixos.org/wiki/Flakes) - the experimental feature `trix` emulates
- [flake-compat](https://github.com/edolstra/flake-compat) - another approach to using flakes without enabling them
- [unflake](https://codeberg.org/goldstein/unflake) - an alternative implementation of a flake dependency resolver and runtime
