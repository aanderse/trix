# trix

> `trix` - trick yourself into flakes

`trix` is an alternative userland implementation of Nix flakes. It reads
`flake.nix` and `flake.lock` files, resolves inputs, and evaluates outputs — all
without requiring you to enable the experimental `flakes` or `nix-command`
features. Under the hood, `trix` delegates to the stable "legacy" Nix commands
(`nix-build`, `nix-shell`, `nix-instantiate`), giving you a modern flake
workflow built on proven foundations.

## Why?

> While `flake.nix` has reached critical mass and there is no going back at this
> point, this doesn't mean we are stuck with the current implementation of
> Flakes.

The primary motivation for `trix` is to address the stagnation of these
"experimental" features. Despite being broadly adopted, `flakes` and
`nix-command` have remained in experimental status for a significant time. This
uncertainty creates a barrier to adoption (where using "experimental" features
is often seen as a liability) and maintains ambiguity regarding their future.
`trix` tries to bridges this gap, treating the Flake **format** as the standard
while relying on the proven stability of traditional Nix internals.

`trix` allows using Nix as if the experimental feature flags `nix-command` and
`flakes` were enabled, without modifying your global configuration. It works by
delegating to stable "legacy" Nix commands (`nix-build`, `nix-shell`,
`nix-instantiate`) whenever possible, rewriting each command to optionally
inject `--extra-experimental-features "flakes nix-command"` when necessary.

This approach offers these benefits:

- **No Global Configuration**: You can keep your `nix.conf` clean of
  `experimental-features = nix-command flakes`.
- **Efficiency**: `trix` evaluates flakes in-place without copying them to the
  Nix store, referencing your working directory directly.
- **Stability**: It leverages the battle-tested legacy Nix tools under the hood.

## Comparison

### vs `nix flake`

| Feature           |         `nix flake`          |                      `trix`                       |
| :---------------- | :--------------------------: | :-----------------------------------------------: |
| **Stability**     |         Experimental         |        **Stable** (uses `nix-build`, etc.)        |
| **Purity**        |           Enforced           |        Optional (allows impure evaluation)        |
| **Performance**   | Copies entire flake to store | **Efficient** (evaluates in-place, no store copy) |
| **Specification** |             Full             |                 Practical subset                  |

### vs plain `nix-build` / `nix-shell`

| Feature                  | Plain Nix |            `trix`            |
| :----------------------- | :-------: | :--------------------------: |
| **`flake.nix` support**  |    ❌     |            **✅**            |
| **Reproducible locking** |  Manual   | **✅ (native `flake.lock`)** |
| **Structured inputs**    |    ❌     |            **✅**            |
| **CLI familiarity**      |  Legacy   |   **Modern (flake-like)**    |

## How it Works

`trix` is a small `rust` tool, written initially in `python` by
[claude](https://claude.ai/) and converted to `rust` with
[Antigravity](https://antigravity.google/) later on, that provides a flake-like
experience using stable Nix commands. It operates as follows:

1. **Introspection**: Parses `flake.nix` to extract input requirements (via
   `nix-instantiate`).
2. **Locking**: Fetches and locks inputs using `nix flake prefetch`. (For remote
   flakes and certain operations like `trix copy`, trix delegates to `nix`
   commands directly.)
3. **Generation**: Produces a standard `flake.lock` file.
4. **Evaluation**: Constructs the inputs attrset and evaluates the flake's
   `outputs`.
5. **Execution**: Delegates the final build or shell action to `nix-build` or
   `nix-shell`.

The resulting lock file is **fully compatible** with using `nix flake`
experimental features. You can even switch between `trix` and official Flakes at
any time without issues.

`trix` translates high-level flake intents into low-level Nix operations. When
running a `trix` command, it actually invokes multiple Nix commands under the
hood to build the desired output. These commands includes the experimental
features flags for you (_some required functions requires flakes to be enabled,
like [NixOS/nix#5541](https://github.com/NixOS/nix/issues/5541)_).

## Direnv Integration

`trix` includes a `direnv` library for seamless environment activation.

1. Add to your `~/.config/direnv/direnvrc`:

   ```bash
   # If installed via nix profile:
   source ~/.nix-profile/share/trix/direnvrc
   # Or with a direct store path:
   source /nix/store/...-trix-0.1.0/share/trix/direnvrc
   ```

2. In your project's `.envrc`:
   ```bash
   use trix
   # or for a specific devShell:
   use trix .#myshell
   ```

## Debugging

`trix` uses structured logging via the `tracing` crate. Diagnostic information
is printed to `stderr` to avoid interfering with command output.

### Verbose Mode

Use the `-v` / `--verbose` flag to enable debug output:

```bash
trix -v build .#default
```

### Environment Variables

You can filter log output granularly using the `RUST_LOG` environment variable.
The default level is `INFO` (or `DEBUG` when `-v` is used).

Examples:

```bash
# different modules at different levels
RUST_LOG=trix=debug,nix=error trix build

# trace everything
RUST_LOG=trace trix build
```

## See Also

- [Nix Flakes](https://wiki.nixos.org/wiki/Flakes): The experimental feature
  `trix` provides an alternative for.
- [flake-compat](https://github.com/edolstra/flake-compat): Makes flake-based
  projects compatible with legacy Nix commands.
- [unflake](https://codeberg.org/goldstein/unflake): An alternative dependency
  resolver & runtime for Nix flakes.
