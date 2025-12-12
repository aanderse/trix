"""CLI entry point for trix."""

import os
import subprocess
import sys
from pathlib import Path

import click

# Import subcommand groups
from .cli_flake import flake
from .cli_profile import profile
from .cli_registry import registry
from .flake import ensure_lock, get_nix_config, parse_installable, resolve_attr_path, resolve_installable
from .nix import (
    _get_clean_env,
    flake_has_attr,
    get_build_log,
    get_derivation_path,
    get_store_path_from_drv,
    get_system,
    run_nix_build,
    run_nix_eval,
    run_nix_repl,
    run_nix_shell,
)


def _passthrough_to_nix(nix_cmd: str, installable: str, extra_args: list[str] = None, verbose: bool = False):
    """Passthrough to nix command for remote flake refs.

    Args:
        nix_cmd: The nix subcommand (build, run, develop, shell)
        installable: The full installable reference (e.g., "nixpkgs#hello")
        extra_args: Extra arguments to pass to nix
        verbose: Whether to print the command
    """
    cmd = ['nix', nix_cmd, '--extra-experimental-features', 'nix-command flakes', installable]
    if extra_args:
        cmd.extend(extra_args)

    if verbose:
        click.echo(f'+ {" ".join(cmd)}', err=True)

    result = subprocess.run(cmd, env=_get_clean_env())
    sys.exit(result.returncode)


@click.group()
@click.version_option()
@click.option('-v', '--verbose', is_flag=True, help='Show commands being run')
@click.pass_context
def cli(ctx, verbose):
    """trix - trick yourself into flakes.

    Operates on flake.nix files using legacy nix-build/nix-shell commands.
    No store copying, no purity restrictions.
    """
    ctx.ensure_object(dict)
    ctx.obj['verbose'] = verbose


# Register subcommand groups
cli.add_command(flake)
cli.add_command(profile)
cli.add_command(registry)


@cli.command()
@click.argument('installable', default='.')
@click.option('-o', '--out-link', default='result', help='Symlink name for result')
@click.option('--no-link', is_flag=True, help="Don't create result symlink")
@click.option('-f', '--file', 'nix_file', help='Build from a Nix file instead of flake.nix')
@click.option(
    '--arg',
    'extra_args',
    multiple=True,
    nargs=2,
    metavar='NAME EXPR',
    help='Pass --arg NAME EXPR to nix-build (can be repeated)',
)
@click.option(
    '--argstr',
    'extra_argstrs',
    multiple=True,
    nargs=2,
    metavar='NAME VALUE',
    help='Pass --argstr NAME VALUE to nix-build (can be repeated)',
)
@click.option('--store', 'store', metavar='URL', help='Use alternative nix store (e.g., local?root=/tmp/store)')
@click.pass_context
def build(ctx, installable, out_link, no_link, nix_file, extra_args, extra_argstrs, store):
    """Build a package from flake.nix or a Nix file.

    \b
    Examples:
      trix build              # Build default package from flake.nix
      trix build .#hello      # Build 'hello' package from flake.nix
      trix build -f default.nix        # Build from default.nix
      trix build -f . hello            # Build 'hello' attr from ./default.nix
      trix build --arg foo 'true'      # Pass --arg to nix-build
      trix build --argstr version 1.0  # Pass --argstr to nix-build
    """
    verbose = ctx.obj['verbose']

    # If -f is specified, bypass flake machinery entirely
    if nix_file:
        cmd = ['nix-build', nix_file]
        # installable becomes the attribute when using -f
        if installable and installable != '.':
            cmd.extend(['-A', installable])

        # Add extra --arg options
        for name, expr in extra_args:
            cmd.extend(['--arg', name, expr])

        # Add extra --argstr options
        for name, value in extra_argstrs:
            cmd.extend(['--argstr', name, value])

        if store:
            cmd.extend(['--store', store])

        if no_link:
            cmd.append('--no-link')
        else:
            cmd.extend(['-o', out_link])

        if verbose:
            print(f'+ {" ".join(cmd)}', file=sys.stderr)

        result = subprocess.run(cmd, env=_get_clean_env())
        if result.returncode != 0:
            sys.exit(result.returncode)
        return

    # Resolve installable - may be local or remote
    resolved = resolve_installable(installable)

    if not resolved.is_local:
        # Passthrough to nix build for remote refs
        nix_args = []
        if no_link:
            nix_args.append('--no-link')
        else:
            nix_args.extend(['-o', out_link])
        if store:
            nix_args.extend(['--store', store])
        # Construct full installable with attr
        full_ref = (
            f'{resolved.flake_ref}#{resolved.attr_part}' if resolved.attr_part != 'default' else resolved.flake_ref
        )
        _passthrough_to_nix('build', full_ref, nix_args, verbose=verbose)
        return

    # Local flake - use trix's native handling
    flake_dir = resolved.flake_dir

    # Ensure lock - this also gets system and caches it
    ensure_lock(flake_dir, verbose=verbose)

    # Now get system (cached from ensure_lock or fetched if needed)
    system = get_system()
    attr = resolve_attr_path(resolved.attr_part, 'packages', system)

    run_nix_build(
        flake_dir=flake_dir,
        attr=attr,
        out_link=None if no_link else out_link,
        verbose=verbose,
        extra_args=list(extra_args) if extra_args else None,
        extra_argstrs=list(extra_argstrs) if extra_argstrs else None,
        store=store,
    )


@cli.command()
@click.argument('installable', default='.')
@click.option('-c', '--command', 'run_cmd', help='Command to run in shell')
@click.option(
    '--arg',
    'extra_args',
    multiple=True,
    nargs=2,
    metavar='NAME EXPR',
    help='Pass --arg NAME EXPR to nix-shell (can be repeated)',
)
@click.option(
    '--argstr',
    'extra_argstrs',
    multiple=True,
    nargs=2,
    metavar='NAME VALUE',
    help='Pass --argstr NAME VALUE to nix-shell (can be repeated)',
)
@click.option('--store', 'store', metavar='URL', help='Use alternative nix store (e.g., local?root=/tmp/store)')
@click.pass_context
def develop(ctx, installable, run_cmd, extra_args, extra_argstrs, store):
    """Enter a development shell from flake.nix.

    \b
    Examples:
      trix develop            # Enter default devShell
      trix develop .#myshell  # Enter 'myshell' devShell
      trix develop --arg foo 'true'      # Pass --arg to nix-shell
      trix develop --argstr env dev      # Pass --argstr to nix-shell
    """
    verbose = ctx.obj['verbose']

    # Resolve installable - may be local or remote
    resolved = resolve_installable(installable)

    if not resolved.is_local:
        # Passthrough to nix develop for remote refs
        nix_args = []
        if run_cmd:
            nix_args.extend(['--command', run_cmd])
        full_ref = (
            f'{resolved.flake_ref}#{resolved.attr_part}' if resolved.attr_part != 'default' else resolved.flake_ref
        )
        _passthrough_to_nix('develop', full_ref, nix_args, verbose=verbose)
        return

    # Local flake - use trix's native handling
    flake_dir = resolved.flake_dir
    ensure_lock(flake_dir, verbose=verbose)

    system = get_system()

    # Try devShells first, fall back to packages (matching nix develop behavior)
    devshell_attr = resolve_attr_path(resolved.attr_part, 'devShells', system)
    package_attr = resolve_attr_path(resolved.attr_part, 'packages', system)

    if flake_has_attr(flake_dir, devshell_attr):
        attr = devshell_attr
    elif flake_has_attr(flake_dir, package_attr):
        attr = package_attr
    else:
        # Neither exists - let it fail with devShells error for clarity
        attr = devshell_attr

    # Get nixConfig for bash prompt settings
    nix_config = get_nix_config(flake_dir)

    run_nix_shell(
        flake_dir=flake_dir,
        attr=attr,
        command=run_cmd,
        verbose=verbose,
        extra_args=list(extra_args) if extra_args else None,
        extra_argstrs=list(extra_argstrs) if extra_argstrs else None,
        store=store,
        bash_prompt=nix_config.get('bash-prompt'),
        bash_prompt_prefix=nix_config.get('bash-prompt-prefix'),
        bash_prompt_suffix=nix_config.get('bash-prompt-suffix'),
    )


@cli.command('eval')
@click.argument('installable', default='.', required=False)
@click.option('--expr', 'expr', metavar='EXPR', help='Evaluate a Nix expression instead of a flake attribute')
@click.option('--json', 'output_json', is_flag=True, help='Output as JSON')
@click.option('--raw', is_flag=True, help='Output raw string without quotes')
@click.option('--apply', 'apply_fn', metavar='EXPR', help='Apply a function to the result')
@click.option(
    '--arg',
    'extra_args',
    multiple=True,
    nargs=2,
    metavar='NAME EXPR',
    help='Pass --arg NAME EXPR to nix-instantiate (can be repeated)',
)
@click.option(
    '--argstr',
    'extra_argstrs',
    multiple=True,
    nargs=2,
    metavar='NAME VALUE',
    help='Pass --argstr NAME VALUE to nix-instantiate (can be repeated)',
)
@click.option('--store', 'store', metavar='URL', help='Use alternative nix store (e.g., local?root=/tmp/store)')
@click.pass_context
def eval_cmd(ctx, installable, expr, output_json, raw, apply_fn, extra_args, extra_argstrs, store):
    """Evaluate a flake attribute or Nix expression and print the result.

    \b
    Examples:
      trix eval .#packages.x86_64-linux.hello.meta.description
      trix eval --json .#packages.x86_64-linux.hello.meta
      trix eval --raw .#packages.x86_64-linux.hello.name
      trix eval .#lib --apply 'lib: lib.version'
      trix eval .#packages.x86_64-linux --apply builtins.attrNames --json
      trix eval --expr '1 + 1'
      trix eval --expr 'builtins.attrNames builtins' --json
    """
    verbose = ctx.obj['verbose']

    if expr:
        # Evaluate raw expression, no flake context needed
        result = run_nix_eval(
            flake_dir=None,
            attr='',
            expr=expr,
            output_json=output_json,
            raw=raw,
            apply_fn=apply_fn,
            verbose=verbose,
            extra_args=list(extra_args) if extra_args else None,
            extra_argstrs=list(extra_argstrs) if extra_argstrs else None,
            store=store,
        )
    else:
        flake_dir, attr_part = parse_installable(installable)
        ensure_lock(flake_dir, verbose=verbose)

        # Pass attr directly - run_nix_eval does fallback search matching nix behavior:
        # tries packages.<system>.<attr>, legacyPackages.<system>.<attr>, then <attr>
        result = run_nix_eval(
            flake_dir=flake_dir,
            attr=attr_part or '',
            output_json=output_json,
            raw=raw,
            apply_fn=apply_fn,
            verbose=verbose,
            extra_args=list(extra_args) if extra_args else None,
            extra_argstrs=list(extra_argstrs) if extra_argstrs else None,
            store=store,
        )

    print(result)


def _try_resolve_app(flake_dir: Path, attr: str, verbose: bool = False) -> str | None:
    """Try to resolve an attribute as an app, returning the program path if found."""
    import json

    try:
        result = run_nix_eval(
            flake_dir=flake_dir,
            attr=attr,
            output_json=True,
            verbose=verbose,
            quiet=True,
        )
        app_data = json.loads(result)
        if isinstance(app_data, dict) and app_data.get('type') == 'app' and 'program' in app_data:
            return app_data['program']
    except (SystemExit, Exception):
        pass
    return None


def _resolve_runnable(flake_dir: Path, attr_part: str, verbose: bool = False) -> tuple[str, str | None]:
    """Resolve an installable to a runnable program.

    Searches in order: apps, packages, legacyPackages (matching nix run behavior).

    Returns:
        (exe_path, attr_used) - exe_path is the program to run, attr_used is the
        attribute that was found (for building packages), or None if it's an app.
    """
    system = get_system()
    name = attr_part if attr_part else 'default'

    # If attr_part already has a category prefix, use it directly
    if attr_part.startswith(('apps.', 'packages.', 'legacyPackages.')):
        category = attr_part.split('.')[0]
        if category == 'apps':
            program = _try_resolve_app(flake_dir, attr_part, verbose)
            if program:
                return (program, None)
        # For packages/legacyPackages, return the attr to build
        return (None, attr_part)

    # Try apps first
    app_attr = f'apps.{system}.{name}'
    program = _try_resolve_app(flake_dir, app_attr, verbose)
    if program:
        return (program, None)

    # Try packages, then legacyPackages
    for category in ('packages', 'legacyPackages'):
        attr = f'{category}.{system}.{name}'
        if flake_has_attr(flake_dir, attr):
            return (None, attr)

    # Nothing found - return packages attr and let build fail with proper error
    return (None, f'packages.{system}.{name}')


@cli.command()
@click.argument('installable', default='.')
@click.argument('args', nargs=-1)
@click.option(
    '--arg',
    'extra_args',
    multiple=True,
    nargs=2,
    metavar='NAME EXPR',
    help='Pass --arg NAME EXPR to nix-build (can be repeated)',
)
@click.option(
    '--argstr',
    'extra_argstrs',
    multiple=True,
    nargs=2,
    metavar='NAME VALUE',
    help='Pass --argstr NAME VALUE to nix-build (can be repeated)',
)
@click.option('--store', 'store', metavar='URL', help='Use alternative nix store (e.g., local?root=/tmp/store)')
@click.pass_context
def run(ctx, installable, args, extra_args, extra_argstrs, store):
    """Build and run a package or app from flake.nix.

    Searches for the program in apps, packages, and legacyPackages
    (matching nix run behavior).

    \b
    Examples:
      trix run                      # Run default app or package
      trix run .#hello              # Run 'hello' app or package
      trix run nixpkgs#hello        # Run 'hello' from nixpkgs (via registry)
      trix run .#hello -- --help    # Pass args to program
      trix run --arg foo 'true' .#hello  # Pass --arg to nix-build
    """
    import os

    verbose = ctx.obj['verbose']

    # Resolve installable - may be local or remote
    resolved = resolve_installable(installable)

    if not resolved.is_local:
        # Passthrough to nix run for remote refs
        nix_args = []
        if args:
            nix_args.append('--')
            nix_args.extend(args)
        full_ref = (
            f'{resolved.flake_ref}#{resolved.attr_part}' if resolved.attr_part != 'default' else resolved.flake_ref
        )
        _passthrough_to_nix('run', full_ref, nix_args, verbose=verbose)
        return

    # Local flake - use trix's native handling
    flake_dir = resolved.flake_dir
    attr_part = resolved.attr_part

    ensure_lock(flake_dir, verbose=verbose)

    # Resolve to either an app program or a package attr to build
    exe_path, pkg_attr = _resolve_runnable(flake_dir, attr_part, verbose=verbose)

    if exe_path:
        # It's an app - run the program directly
        if verbose:
            click.echo(f'+ {exe_path} {" ".join(args)}', err=True)
        os.execv(exe_path, [exe_path] + list(args))
    else:
        # Build the package
        store_path = run_nix_build(
            flake_dir=flake_dir,
            attr=pkg_attr,
            out_link=None,
            verbose=verbose,
            capture_output=True,
            extra_args=list(extra_args) if extra_args else None,
            extra_argstrs=list(extra_argstrs) if extra_argstrs else None,
            store=store,
        )

        if not store_path:
            click.echo('Build produced no output', err=True)
            sys.exit(1)

        # Find executable
        bin_dir = Path(store_path) / 'bin'
        if not bin_dir.is_dir():
            click.echo(f'No bin directory in {store_path}', err=True)
            sys.exit(1)

        executables = list(bin_dir.iterdir())
        if not executables:
            click.echo(f'No executables in {bin_dir}', err=True)
            sys.exit(1)

        # Use executable matching package name, or first one
        pkg_name = attr_part.split('.')[-1] if attr_part else 'default'
        exe = None
        for e in executables:
            if e.name == pkg_name:
                exe = e
                break
        if exe is None:
            exe = executables[0]

        if verbose:
            click.echo(f'+ {exe} {" ".join(args)}', err=True)

        os.execv(str(exe), [str(exe)] + list(args))


@cli.command()
@click.argument('installable', default='.')
@click.option('--to', required=True, help='Destination store URI (e.g., ssh://user@host)')
@click.option('--no-check-sigs', is_flag=True, help='Do not require that paths are signed by trusted keys')
@click.pass_context
def copy(ctx, installable, to, no_check_sigs):
    """Copy a package to another store.

    \b
    Examples:
      trix copy --to ssh://user@host         # Copy default package
      trix copy .#hello --to ssh://user@host # Copy 'hello' package
      trix copy --to s3://my-cache           # Copy to S3 binary cache
      trix copy --to ssh-ng://host --no-check-sigs  # Copy without signature check
    """
    verbose = ctx.obj['verbose']

    # Resolve installable - may be local or remote
    resolved = resolve_installable(installable)

    if not resolved.is_local:
        # For remote refs, we need to build first then copy the store path
        # Use nix build to get store path, then copy
        full_ref = (
            f'{resolved.flake_ref}#{resolved.attr_part}' if resolved.attr_part != 'default' else resolved.flake_ref
        )
        cmd = [
            'nix',
            'build',
            '--extra-experimental-features',
            'nix-command flakes',
            '--no-link',
            '--print-out-paths',
            full_ref,
        ]
        if verbose:
            click.echo(f'+ {" ".join(cmd)}', err=True)
        result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
        if result.returncode != 0:
            click.echo(f'Failed to build {full_ref}', err=True)
            click.echo(result.stderr, err=True)
            sys.exit(result.returncode)
        store_path = result.stdout.strip()
    else:
        # Local flake - use trix's native handling
        flake_dir = resolved.flake_dir
        ensure_lock(flake_dir, verbose=verbose)

        system = get_system()
        attr = resolve_attr_path(resolved.attr_part, 'packages', system)

        # Build and get store path
        store_path = run_nix_build(
            flake_dir=flake_dir,
            attr=attr,
            out_link=None,
            verbose=verbose,
            capture_output=True,
        )

    if not store_path:
        click.echo('Build produced no output', err=True)
        sys.exit(1)

    # Copy to destination
    if to.startswith('ssh://') and not no_check_sigs:
        # Use stable nix-copy-closure for SSH (doesn't support --no-check-sigs)
        host = to[6:]
        cmd = ['nix-copy-closure', '--to', host, store_path]
    else:
        # Use nix copy for other destinations or when --no-check-sigs is needed
        cmd = ['nix', 'copy', '--extra-experimental-features', 'nix-command', '--to', to, store_path]
        if no_check_sigs:
            cmd.append('--no-check-sigs')

    if verbose:
        click.echo(f'+ {" ".join(cmd)}', err=True)

    result = subprocess.run(cmd, env=_get_clean_env())
    if result.returncode != 0:
        sys.exit(result.returncode)

    click.echo(f'Copied {store_path} to {to}')


@cli.command()
@click.argument('installables', nargs=-1)
@click.option('-c', '--command', 'run_cmd', help='Command to run in shell')
@click.pass_context
def shell(ctx, installables, run_cmd):
    """Start a shell with specified packages available.

    Unlike 'develop' which enters a devShell, 'shell' adds specific
    packages to your PATH temporarily.

    \b
    Examples:
      trix shell .#hello              # Shell with hello package
      trix shell nixpkgs#cowsay       # Shell with cowsay from nixpkgs
      trix shell .#hello .#cowsay     # Shell with multiple packages
      trix shell .#hello -c 'hello'   # Run command and exit
    """
    import os

    verbose = ctx.obj['verbose']

    if not installables:
        click.echo('No packages specified', err=True)
        sys.exit(1)

    # Check if any installables are remote - if so, passthrough all to nix shell
    has_remote = False
    for installable in installables:
        resolved = resolve_installable(installable)
        if not resolved.is_local:
            has_remote = True
            break

    if has_remote:
        # Passthrough to nix shell for remote refs
        nix_args = list(installables)
        if run_cmd:
            nix_args.extend(['--command', run_cmd])
        cmd = ['nix', 'shell', '--extra-experimental-features', 'nix-command flakes'] + nix_args
        if verbose:
            click.echo(f'+ {" ".join(cmd)}', err=True)
        result = subprocess.run(cmd, env=_get_clean_env())
        sys.exit(result.returncode)

    # All local - use trix's native handling
    # Build all packages and collect their paths
    store_paths = []
    for installable in installables:
        resolved = resolve_installable(installable)
        flake_dir = resolved.flake_dir
        ensure_lock(flake_dir, verbose=verbose)

        system = get_system()
        attr = resolve_attr_path(resolved.attr_part, 'packages', system)

        store_path = run_nix_build(
            flake_dir=flake_dir,
            attr=attr,
            out_link=None,
            verbose=verbose,
            capture_output=True,
        )

        if store_path:
            store_paths.append(store_path)
        else:
            click.echo(f'Failed to build {installable}', err=True)
            sys.exit(1)

    # Build PATH with all package bin directories
    bin_paths = []
    for store_path in store_paths:
        bin_dir = Path(store_path) / 'bin'
        if bin_dir.is_dir():
            bin_paths.append(str(bin_dir))

    if not bin_paths:
        click.echo('No bin directories found in packages', err=True)
        sys.exit(1)

    # Prepend to existing PATH
    new_path = ':'.join(bin_paths)
    if os.environ.get('PATH'):
        new_path = new_path + ':' + os.environ['PATH']

    env = _get_clean_env()
    env['PATH'] = new_path

    if run_cmd:
        # Run command and exit
        if verbose:
            click.echo(f'+ {run_cmd}', err=True)
        result = subprocess.run(run_cmd, shell=True, env=env)
        sys.exit(result.returncode)
    else:
        # Start interactive shell
        shell_cmd = os.environ.get('SHELL', '/bin/sh')
        if verbose:
            click.echo(f'+ {shell_cmd} (with packages in PATH)', err=True)
        os.execvpe(shell_cmd, [shell_cmd], env)


@cli.command()
@click.argument('flake_ref', required=False, default=None)
@click.pass_context
def repl(ctx, flake_ref):
    """Start an interactive Nix REPL.

    Without arguments, starts a plain nix repl.
    With a flake reference, loads the flake's per-system outputs.

    \b
    Examples:
      trix repl              # Plain nix repl
      trix repl .#           # Load current directory's flake
      trix repl /path/to/flake  # Load specific flake
    """
    verbose = ctx.obj['verbose']

    # No argument = plain nix repl
    if flake_ref is None:
        cmd = ['nix', 'repl']
        if verbose:
            click.echo(f'+ {" ".join(cmd)}', err=True)
        os.execvp(cmd[0], cmd)

    # Parse flake reference - strip trailing # if present
    flake_path = flake_ref.rstrip('#')
    if flake_path == '.' or flake_path == '':
        flake_dir = Path.cwd()
    else:
        flake_dir = Path(flake_path).resolve()

    if not (flake_dir / 'flake.nix').exists():
        click.echo(f'No flake.nix found in {flake_dir}', err=True)
        sys.exit(1)

    ensure_lock(flake_dir, verbose=verbose)

    run_nix_repl(
        flake_dir=flake_dir,
        verbose=verbose,
    )


@cli.command()
@click.argument('installable', default='.')
@click.pass_context
def log(ctx, installable):
    """Show build log for a package.

    Displays the build log from the last build of the specified package.
    The log is retrieved from the local store or binary cache.

    \b
    Examples:
      trix log              # Show log for default package
      trix log .#hello      # Show log for 'hello' package
    """
    verbose = ctx.obj['verbose']
    flake_dir, attr_part = parse_installable(installable)

    ensure_lock(flake_dir, verbose=verbose)

    system = get_system()
    attr = resolve_attr_path(attr_part, 'packages', system)

    # Get derivation path (without building)
    drv_path = get_derivation_path(flake_dir, attr, verbose=verbose)

    # Get store path from derivation
    store_path = get_store_path_from_drv(drv_path, verbose=verbose)

    # Get build log
    log_content = get_build_log(store_path, verbose=verbose)

    if log_content:
        click.echo(log_content)
    else:
        # Try getting log from derivation path directly
        log_content = get_build_log(drv_path, verbose=verbose)
        if log_content:
            click.echo(log_content)
        else:
            click.echo(f'No build log available for {store_path}', err=True)
            click.echo('The package may have been fetched from a binary cache or not yet built.', err=True)
            sys.exit(1)


@cli.command('why-depends')
@click.argument('package')
@click.argument('dependency')
@click.pass_context
def why_depends(ctx, package, dependency):
    """Show why a package depends on another.

    Traces the dependency chain from PACKAGE to DEPENDENCY,
    showing why DEPENDENCY is in PACKAGE's closure.

    Both arguments can be installables (.#foo) or store paths.

    \b
    Examples:
      trix why-depends .#hello .#glibc
      trix why-depends .#trix /nix/store/xxx-glibc-2.40
    """
    verbose = ctx.obj['verbose']

    def resolve_to_store_path(ref: str) -> str:
        """Resolve an installable or store path to a store path."""
        if ref.startswith('/nix/store/'):
            return ref

        flake_dir, attr_part = parse_installable(ref)
        ensure_lock(flake_dir, verbose=verbose)
        system = get_system()
        attr = resolve_attr_path(attr_part, 'packages', system)

        # Build to get store path (needed for why-depends to work)
        store_path = run_nix_build(
            flake_dir=flake_dir,
            attr=attr,
            out_link=None,
            verbose=verbose,
            capture_output=True,
        )
        if not store_path:
            click.echo(f'Failed to build {ref}', err=True)
            sys.exit(1)
        return store_path

    pkg_path = resolve_to_store_path(package)
    dep_path = resolve_to_store_path(dependency)

    cmd = ['nix', 'why-depends', '--extra-experimental-features', 'nix-command', pkg_path, dep_path]

    if verbose:
        click.echo(f'+ {" ".join(cmd)}', err=True)

    result = subprocess.run(cmd, env=_get_clean_env())
    sys.exit(result.returncode)


@cli.command()
@click.argument('shell', type=click.Choice(['bash', 'zsh', 'fish']))
def completion(shell):
    """Generate shell completion script.

    \b
    Install completions:
      bash: trix completion bash >> ~/.bashrc
      zsh:  trix completion zsh >> ~/.zshrc
      fish: trix completion fish > ~/.config/fish/completions/trix.fish
    """
    from click.shell_completion import get_completion_class

    comp_cls = get_completion_class(shell)
    comp = comp_cls(cli, {}, 'trix', '_TRIX_COMPLETE')
    script = comp.source()
    click.echo(script)


def main():
    cli()


if __name__ == '__main__':
    main()
