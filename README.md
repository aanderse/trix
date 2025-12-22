# trix

> `trix` - trick yourself into flakes

`trix` is an alternative userland implementation of Nix flakes. It reads
`flake.nix` and `flake.lock` files, resolves inputs, and evaluates outputs—all
without requiring you to enable the experimental `flakes` or `nix-command`
features. Under the hood, `trix` delegates to the stable "legacy" Nix commands
(`nix-build`, `nix-shell`, `nix-instantiate`), giving you a modern flake
workflow built on proven foundations.

## Why?

> While `flake.nix` has reached critical mass and there is no going back at this
> point, this doesn't mean we are stuck with the current implementation of
> Flakes.

`trix` acknowledges that the Flake _format_ is the future of Nix packaging, but
provides an alternative _implementation_ that relies on the proven stability of
traditional Nix tools.

The primary motivation for `trix` is efficiency: `trix` will never inadvertently
copy your local flakes to the Nix store. It evaluates flakes in-place,
referencing your working directory directly, which keeps commands fast even in
large repositories.

Additionally, `trix` lets you use flakes without enabling experimental features:

- **`nix-command`**: `trix` provides a modern CLI experience (`trix build`,
  `trix develop`, `trix run`) but delegates to stable legacy tools under the
  hood.
- **`flakes`**: `trix` implements its own flake resolver and evaluator, so you
  can use `flake.nix` and `flake.lock` with any Nix installation.

With `trix`, you can keep your `nix.conf` clean of
`experimental-features = nix-command flakes`.

## Comparison

### vs `nix flake`

| Feature           |         `nix flake`          |                    `trix`                    |
| :---------------- | :--------------------------: | :------------------------------------------: |
| **Stability**     |         Experimental         |     **Stable** (uses `nix-build`, etc.)      |
| **Purity**        |           Enforced           |     Optional (allows impure evaluation)      |
| **Performance**   | Copies entire flake to store | **Efficient** (evaluates in-place, no store copy) |
| **Specification** |             Full             |               Practical subset               |

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

The resulting lock file is **fully compatible** with `nix flake` — you can
switch between `trix` and official Flakes at any time.

### Examples

- `trix develop [<path>#attr]`: Runs
  `nix-shell <trix-path>/eval.nix --argstr attr devShells.<system>.<attr>`,
  where `<system>` is the current system (e.g., `x86_64-linux`).
- `trix build [<path>#attr]`: Runs
  `nix-build <trix-path>/eval.nix --argstr attr packages.<system>.<attr> -o result`.
- `trix run [<path>#attr]`: Builds the package (or app) and executes the
  resulting binary.

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

`trix` uses structured logging via the `tracing` crate. Diagnostic information is
printed to `stderr` to avoid interfering with command output.

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

- [Nix Flakes](https://wiki.nixos.org/wiki/Flakes) - The experimental feature
  `trix` provides an alternative for.
- [flake-compat](https://github.com/edolstra/flake-compat) - Makes flake-based
  projects compatible with legacy Nix commands.
- [unflake](https://codeberg.org/goldstein/unflake) - An alternative
  implementation of a flake dependency resolver and runtime.
