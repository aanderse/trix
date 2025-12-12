"""CLI commands for flake management."""

import json
import subprocess
import sys
from pathlib import Path

import click

from .flake import ensure_lock, get_flake_description, get_flake_inputs
from .lock import update_lock
from .nix import (
    _get_clean_env,
    eval_flake_outputs,
    eval_flake_outputs_parallel,
    get_nix_dir,
    get_system,
    run_nix_build,
)


def _format_input_url(node: dict) -> str:
    """Format a locked input node as a flake URL (native flake.lock format)."""
    locked = node.get('locked', {})
    typ = locked.get('type', '')
    if typ == 'github':
        owner = locked.get('owner', '')
        repo = locked.get('repo', '')
        rev = locked.get('rev', '')
        return f'github:{owner}/{repo}/{rev}'
    elif typ == 'git':
        url = locked.get('url', '')
        rev = locked.get('rev', '')
        return f'git+{url}?rev={rev}'
    elif typ == 'path':
        return f'path:{locked.get("path", "")}'
    elif typ == 'tarball':
        return locked.get('url', '')
    else:
        return locked.get('url', typ)


def _bold(text: str) -> str:
    """Wrap text in ANSI bold codes."""
    return f'\033[1m{text}\033[0m'


def _green_bold(text: str) -> str:
    """Wrap text in green bold ANSI codes (for tree characters)."""
    return f'\033[32;1m{text}\033[0m'


def _magenta_bold(text: str) -> str:
    """Wrap text in magenta bold ANSI codes (for 'omitted')."""
    return f'\033[35;1m{text}\033[0m'


def _format_timestamp(ts: int | None) -> str:
    """Format a Unix timestamp as a human-readable date."""
    if ts is None:
        return ''
    from datetime import datetime

    return datetime.fromtimestamp(ts).strftime('%Y-%m-%d %H:%M:%S')


def _get_output_description(category: str, name: str, info: dict | str | None = None) -> str:
    """Get a human-readable description for an output.

    Args:
        category: Output category (packages, devShells, etc.)
        name: Attribute name
        info: Either a dict with _type/_name/_omitted/_legacyOmitted/_unknown, or "leaf", or None
    """
    # Check for omitted (non-current system)
    if isinstance(info, dict) and info.get('_omitted'):
        return f"{_magenta_bold('omitted')} (use '--all-systems' to show)"

    # Check for legacyPackages omitted (too large to enumerate)
    if isinstance(info, dict) and info.get('_legacyOmitted'):
        return f"{_magenta_bold('omitted')} (use '--legacy' to show)"

    # Check for unknown category (lib, htmlDocs, etc.)
    if isinstance(info, dict) and info.get('_unknown'):
        return _magenta_bold('unknown')

    # Check for typed entries (modules, templates, overlays, configurations, formatter)
    if isinstance(info, dict):
        entry_type = info.get('_type')
        if entry_type == 'module':
            return _magenta_bold('NixOS module')
        elif entry_type == 'template':
            return _magenta_bold('template')
        elif entry_type == 'overlay':
            return _magenta_bold('overlay')
        elif entry_type == 'configuration':
            return _magenta_bold('configuration')
        elif entry_type == 'formatter':
            return 'package'  # formatter is a package derivation

    # Get base description for derivation-based categories
    if category == 'devShells':
        # devShells don't show derivation name, just the static description
        return "development environment 'nix-shell'"
    elif category == 'nixosConfigurations':
        return 'NixOS configuration'
    elif category == 'packages':
        base = 'package'
    elif category == 'apps':
        base = 'app'
    elif category == 'checks':
        base = 'check'
    elif category == 'formatter':
        base = 'package'
    elif category == 'overlays':
        return 'overlay'
    elif category == 'nixosModules':
        return 'NixOS module'
    elif category == 'homeConfigurations':
        return 'Home Manager configuration'
    else:
        base = ''

    # Add derivation name if available (for packages, apps, checks, formatter)
    if isinstance(info, dict) and info.get('_type') == 'derivation':
        if info.get('_name'):
            return f"{base} '{info['_name']}'"
        return base or 'derivation'

    return base


@click.group()
@click.pass_context
def flake(ctx):
    """Manage flake inputs and outputs.

    \b
    Examples:
      trix flake show
      trix flake check
      trix flake metadata
      trix flake update
      trix flake lock
    """
    pass


@flake.command()
@click.argument('flake_ref', default='.')
@click.pass_context
def metadata(ctx, flake_ref):
    """Show flake metadata and inputs.

    \b
    Examples:
      trix flake metadata       # Show metadata for current flake
      trix flake metadata .     # Same as above
    """
    verbose = ctx.obj['verbose']
    flake_dir = Path(flake_ref).resolve() if flake_ref != '.' else Path.cwd()

    flake_nix = flake_dir / 'flake.nix'
    if not flake_nix.exists():
        click.echo(f'No flake.nix found in {flake_dir}', err=True)
        sys.exit(1)

    # Note: Unlike `flake show`, metadata is read-only and doesn't modify the lock file.
    # This matches `nix flake metadata` behavior.

    # Get description from flake.nix
    description = get_flake_description(flake_dir)
    if description:
        click.echo(f'{_bold("Description:")}   {description}')

    click.echo(f'{_bold("Path:")}          {flake_dir}')

    # Last modified from flake.nix mtime
    mtime = int(flake_nix.stat().st_mtime)
    click.echo(f'{_bold("Last modified:")} {_format_timestamp(mtime)}')

    # Read lock file
    lock_file = flake_dir / 'flake.lock'
    if lock_file.exists():
        with open(lock_file) as f:
            lock = json.load(f)

        nodes = lock.get('nodes', {})
        root_inputs = nodes.get('root', {}).get('inputs', {})

        def print_input(name: str, node_ref: str, prefix: str, is_last: bool) -> None:
            """Print an input and its transitive dependencies."""
            branch = '└───' if is_last else '├───'
            node = nodes.get(node_ref, {})
            url = _format_input_url(node)
            # Add timestamp if available
            last_mod = node.get('locked', {}).get('lastModified')
            if last_mod:
                url += f' ({_format_timestamp(last_mod)})'
            click.echo(f'{prefix}{branch}{_bold(name)}: {url}')

            # Print transitive inputs
            node_inputs = node.get('inputs', {})
            if node_inputs:
                child_prefix = prefix + ('    ' if is_last else '│   ')
                input_names = sorted(node_inputs.keys())
                for j, child_name in enumerate(input_names):
                    child_ref = node_inputs[child_name]
                    # Handle .follows references (lists like ["nixpkgs", "nixpkgs"])
                    if isinstance(child_ref, list):
                        child_is_last = j == len(input_names) - 1
                        child_branch = '└───' if child_is_last else '├───'
                        follows_path = '/'.join(child_ref)
                        click.echo(f"{child_prefix}{child_branch}{_bold(child_name)} follows input '{follows_path}'")
                    else:
                        print_input(child_name, child_ref, child_prefix, j == len(input_names) - 1)

        if root_inputs:
            click.echo(f'{_bold("Inputs:")}')
            names = sorted(root_inputs.keys())
            for i, name in enumerate(names):
                node_ref = root_inputs[name]
                print_input(name, node_ref, '', i == len(names) - 1)
    else:
        inputs = get_flake_inputs(flake_dir)
        if inputs:
            click.echo(f'{_bold("Inputs (unlocked):")}')
            names = sorted(inputs.keys())
            for i, name in enumerate(names):
                spec = inputs[name]
                is_last = i == len(names) - 1
                prefix = '└───' if is_last else '├───'
                if spec.get('type') == 'github':
                    owner = spec.get('owner', '')
                    repo = spec.get('repo', '')
                    ref = spec.get('ref', '')
                    url = f'github:{owner}/{repo}' + (f'/{ref}' if ref else '')
                elif spec.get('type') == 'path':
                    url = f'path:{spec.get("path", "")}'
                elif spec.get('type') == 'git':
                    url = f'git+{spec.get("url", "")}'
                else:
                    url = str(spec)
                click.echo(f'{prefix}{_bold(name)}: {url}')


def _get_flake_url(flake_dir: Path) -> str:
    """Get the flake URL for display (matching nix's format).

    Note: We display git+file:// for git repos to match nix's output format,
    even though trix doesn't implement git-aware filtering (respecting .gitignore).
    This is a known limitation - trix passes the directory directly to nix-instantiate.
    If git filtering becomes important, we'd need to implement proper git tree handling.
    """
    # Check if it's a git repo
    git_dir = flake_dir / '.git'
    if git_dir.exists():
        return f'git+file://{flake_dir}'
    else:
        return f'path:{flake_dir}'


def _is_remote_flake_ref(ref: str) -> bool:
    """Check if a flake reference is remote (not a local path)."""
    # Remote refs contain : (github:, git+, path:, etc.) or are registry names
    if ':' in ref:
        # path: is local, everything else is remote
        return not ref.startswith('path:')
    # Check for explicit local paths
    if ref in ('.', '') or ref.startswith(('/', './', '../', '~')):
        return False
    # If it looks like a path that exists, it's local
    if Path(ref).exists():
        return False
    # Otherwise assume it's a registry name (nixpkgs, etc.)
    return True


@flake.command()
@click.argument('flake_ref', default='.')
@click.option('--all-systems', is_flag=True, help='Show outputs for all systems, not just the current one.')
@click.option('--legacy', is_flag=True, help='Show legacyPackages contents (can be slow for large package sets).')
@click.option('-j', '--jobs', default=4, type=int, help='Number of parallel evaluations (default: 4, use 1 to disable).')
@click.pass_context
def show(ctx, flake_ref, all_systems, legacy, jobs):
    """Show flake outputs structure.

    \b
    Examples:
      trix flake show               # Show outputs for current flake
      trix flake show .             # Same as above
      trix flake show --all-systems # Show all systems (not just current)
      trix flake show --legacy      # Include legacyPackages contents
      trix flake show -j1           # Disable parallel evaluation
      trix flake show github:owner/repo  # Show remote flake (via nix)
    """
    verbose = ctx.obj['verbose']

    # Check if this is a remote flake ref - passthrough to nix
    if _is_remote_flake_ref(flake_ref):
        cmd = ['nix', '--extra-experimental-features', 'nix-command flakes', 'flake', 'show', flake_ref]
        if all_systems:
            cmd.append('--all-systems')
        if legacy:
            cmd.append('--legacy')
        if verbose:
            click.echo(f'+ {" ".join(cmd)}', err=True)
        result = subprocess.run(cmd, env=_get_clean_env())
        sys.exit(result.returncode)

    flake_dir = Path(flake_ref).resolve() if flake_ref != '.' else Path.cwd()

    if not (flake_dir / 'flake.nix').exists():
        click.echo(f'No flake.nix found in {flake_dir}', err=True)
        sys.exit(1)

    ensure_lock(flake_dir, verbose=verbose)

    # Get outputs structure (parallel or sequential)
    if jobs > 1:
        outputs = eval_flake_outputs_parallel(
            flake_dir, verbose=verbose, all_systems=all_systems, show_legacy=legacy, jobs=jobs
        )
    else:
        outputs = eval_flake_outputs(flake_dir, verbose=verbose, all_systems=all_systems, show_legacy=legacy)
    if not outputs:
        click.echo('No outputs found', err=True)
        sys.exit(1)

    # Print flake URL at the top (bold)
    print(_bold(_get_flake_url(flake_dir)))

    system = get_system()
    per_system_attrs = {'packages', 'devShells', 'checks', 'apps', 'legacyPackages', 'formatter'}
    categories = sorted(outputs.keys())

    for cat_idx, category in enumerate(categories):
        contents = outputs[category]
        is_last_cat = cat_idx == len(categories) - 1
        cat_branch = _green_bold('└───' if is_last_cat else '├───')

        # Check if the entire category is marked as unknown (lib, htmlDocs, etc.)
        if isinstance(contents, dict) and contents.get('_unknown'):
            print(f'{cat_branch}{_bold(category)}: {_magenta_bold("unknown")}')
            continue

        print(f'{cat_branch}{_bold(category)}')

        if not isinstance(contents, dict):
            continue

        # Continuation prefix for children (green bold for tree chars)
        child_prefix = _green_bold('    ' if is_last_cat else '│   ')

        # Check if it's system-keyed (packages, devShells, apps, checks, etc.)
        if category in per_system_attrs or category == 'formatter':
            # Show all systems, sorted with current system's position preserved
            systems = sorted(contents.keys())
            for sys_idx, sys_name in enumerate(systems):
                sys_contents = contents[sys_name]
                is_last_sys = sys_idx == len(systems) - 1
                sys_branch = _green_bold('└───' if is_last_sys else '├───')

                # Check if this system's contents were omitted (legacyPackages)
                if isinstance(sys_contents, dict) and sys_contents.get('_legacyOmitted'):
                    desc = _get_output_description(category, sys_name, sys_contents)
                    print(f'{child_prefix}{sys_branch}{_bold(sys_name)}: {desc}')
                    continue

                # For formatter: system maps directly to derivation, not {name -> derivation}
                if category == 'formatter':
                    desc = _get_output_description(category, sys_name, sys_contents)
                    print(f'{child_prefix}{sys_branch}{_bold(sys_name)}: {desc}')
                    continue

                print(f'{child_prefix}{sys_branch}{_bold(sys_name)}')

                if isinstance(sys_contents, dict):
                    inner_prefix = child_prefix + _green_bold('    ' if is_last_sys else '│   ')
                    names = sorted(sys_contents.keys())
                    for i, name in enumerate(names):
                        is_last = i == len(names) - 1
                        branch = _green_bold('└───' if is_last else '├───')
                        info = sys_contents.get(name)
                        desc = _get_output_description(category, name, info)
                        line = f'{inner_prefix}{branch}{_bold(name)}'
                        if desc:
                            line += f': {desc}'
                        print(line)
        else:
            # Not system-keyed (nixosConfigurations, overlays, etc.)
            keys = sorted(contents.keys())
            for i, key in enumerate(keys):
                is_last = i == len(keys) - 1
                branch = _green_bold('└───' if is_last else '├───')
                info = contents.get(key)
                desc = _get_output_description(category, key, info)
                line = f'{child_prefix}{branch}{_bold(key)}'
                if desc:
                    line += f': {desc}'
                print(line)


@flake.command()
@click.argument('input_name', required=False)
@click.option(
    '--override-input',
    '-o',
    multiple=True,
    nargs=2,
    metavar='NAME REF',
    help='Pin an input to a specific flake reference',
)
@click.pass_context
def update(ctx, input_name, override_input):
    """Update flake.lock to latest versions.

    \b
    Examples:
      trix flake update                     # Update all inputs
      trix flake update nixpkgs             # Update only nixpkgs
      trix flake update -o nixpkgs github:NixOS/nixpkgs/abc123
                                            # Pin nixpkgs to specific commit
      trix flake update -o nixpkgs nixos-24.05
                                            # Pin nixpkgs to a branch/tag
    """
    verbose = ctx.obj['verbose']
    flake_dir = Path.cwd()

    if not (flake_dir / 'flake.nix').exists():
        click.echo('No flake.nix found in current directory', err=True)
        sys.exit(1)

    # Build override_inputs dict from --override-input options
    override_inputs = {}
    for name, ref in override_input:
        # If ref doesn't look like a full flake ref, assume it's a branch/tag for nixpkgs-like inputs
        if ':' not in ref and '/' not in ref:
            # Get the input spec to construct a proper flake ref
            inputs = get_flake_inputs(flake_dir)
            if name in inputs:
                spec = inputs[name]
                if spec.get('type') == 'github':
                    ref = f'github:{spec["owner"]}/{spec["repo"]}/{ref}'
                elif spec.get('type') == 'git':
                    ref = f'git+{spec["url"]}?ref={ref}'
        override_inputs[name] = ref

    changes = update_lock(
        flake_dir,
        input_name=input_name,
        override_inputs=override_inputs if override_inputs else None,
        verbose=verbose,
    )
    if changes is None:
        sys.exit(1)


@flake.command()
@click.argument('flake_ref', default='.')
@click.pass_context
def lock(ctx, flake_ref):
    """Create or update flake.lock without building.

    \b
    Examples:
      trix flake lock       # Lock current flake
      trix flake lock .     # Same as above
    """
    verbose = ctx.obj['verbose']
    flake_dir = Path(flake_ref).resolve() if flake_ref != '.' else Path.cwd()

    if not (flake_dir / 'flake.nix').exists():
        click.echo(f'No flake.nix found in {flake_dir}', err=True)
        sys.exit(1)

    ensure_lock(flake_dir, verbose=verbose)
    # Note: sync_inputs prints changes to stderr, so no need to print here


def _build_check(args: tuple) -> tuple[str, bool, str]:
    """Build a single check. Returns (name, success, error_msg)."""
    name, flake_dir, attr, verbose = args
    try:
        store_path = run_nix_build(
            flake_dir=flake_dir,
            attr=attr,
            out_link=None,
            verbose=verbose,
            capture_output=True,
        )
        if store_path:
            return (name, True, '')
        else:
            return (name, False, 'no output')
    except SystemExit:
        return (name, False, 'build failed')
    except Exception as e:
        return (name, False, str(e))


@flake.command()
@click.argument('flake_ref', default='.')
@click.option('-j', '--jobs', default=1, type=int, help='Number of parallel builds (default: 1)')
@click.pass_context
def check(ctx, flake_ref, jobs):
    """Run flake checks.

    Evaluates flake outputs and builds all checks.${system}.* derivations.
    Use -j/--jobs to run builds in parallel.

    \b
    Examples:
      trix flake check                    # Run checks for current flake
      trix flake check .                  # Same as above
      trix flake check -j4                # Run 4 checks in parallel
      trix flake check /path/to/flake
      trix flake check github:owner/repo  # Check remote flake (via nix)
    """
    verbose = ctx.obj['verbose']

    # Check if this is a remote flake ref - passthrough to nix
    if _is_remote_flake_ref(flake_ref):
        cmd = ['nix', '--extra-experimental-features', 'nix-command flakes', 'flake', 'check', flake_ref]
        if verbose:
            click.echo(f'+ {" ".join(cmd)}', err=True)
        result = subprocess.run(cmd, env=_get_clean_env())
        sys.exit(result.returncode)

    flake_dir = Path(flake_ref).resolve() if flake_ref != '.' else Path.cwd()

    if not (flake_dir / 'flake.nix').exists():
        click.echo(f'No flake.nix found in {flake_dir}', err=True)
        sys.exit(1)

    ensure_lock(flake_dir, verbose=verbose)

    system = get_system()

    # Get flake outputs structure for validation
    click.echo('Evaluating flake...', err=True)
    outputs = eval_flake_outputs(flake_dir, verbose=verbose)

    if not outputs:
        click.echo('Failed to evaluate flake outputs', err=True)
        sys.exit(1)

    eval_errors = []

    # Validate output types (like nix flake check does)
    # These should be derivations
    derivation_outputs = ['packages', 'devShells', 'checks']
    for category in derivation_outputs:
        if category in outputs and system in outputs[category]:
            for name, info in outputs[category][system].items():
                if isinstance(info, dict) and info.get('_type') != 'derivation':
                    eval_errors.append(f'{category}.{system}.{name} is not a derivation')

    # Check apps have correct structure
    if 'apps' in outputs and system in outputs['apps']:
        for name, info in outputs['apps'][system].items():
            if isinstance(info, dict) and info.get('_type') not in ('app', None):
                # Apps should have type "app" or be a derivation-like thing
                pass  # We don't have full app validation yet

    # Check templates
    if 'templates' in outputs:
        for name, info in outputs['templates'].items():
            if isinstance(info, dict) and not info.get('_omitted'):
                # Templates should have 'path' attribute
                pass  # We don't have full template validation yet

    if eval_errors:
        click.echo('Evaluation errors:', err=True)
        for err in eval_errors:
            click.echo(f'  - {err}', err=True)
        sys.exit(1)

    # Now build the checks
    if 'checks' not in outputs:
        click.echo('No checks defined in flake')
        sys.exit(0)

    checks_output = outputs.get('checks', {})
    if system not in checks_output:
        click.echo(f'No checks for system {system}')
        sys.exit(0)

    check_names = list(checks_output[system].keys())
    if not check_names:
        click.echo('No checks to run')
        sys.exit(0)

    click.echo(f'Running {len(check_names)} check(s)' + (f' with {jobs} job(s)...' if jobs > 1 else '...'))

    failed = []
    passed = []

    # Build check arguments
    build_args = [
        (name, flake_dir, f'checks.{system}.{name}', verbose)
        for name in check_names
    ]

    if jobs > 1:
        # Parallel execution with ThreadPoolExecutor
        from concurrent.futures import ThreadPoolExecutor, as_completed

        with ThreadPoolExecutor(max_workers=jobs) as executor:
            futures = {executor.submit(_build_check, args): args[0] for args in build_args}
            for future in as_completed(futures):
                name, success, error_msg = future.result()
                if success:
                    passed.append(name)
                    click.echo(f'  ✓ {name}')
                else:
                    failed.append(name)
                    click.echo(f'  ✗ {name}' + (f' ({error_msg})' if error_msg else ''))
    else:
        # Sequential execution
        for args in build_args:
            name, success, error_msg = _build_check(args)
            if success:
                passed.append(name)
                click.echo(f'  ✓ {name}')
            else:
                failed.append(name)
                click.echo(f'  ✗ {name}' + (f' ({error_msg})' if error_msg else ''))

    # Summary
    click.echo()
    if failed:
        click.echo(f'Failed: {len(failed)}/{len(check_names)} checks')
        for name in sorted(failed):
            click.echo(f'  - {name}')
        sys.exit(1)
    else:
        click.echo(f'Passed: {len(passed)}/{len(check_names)} checks')


@flake.command()
@click.option(
    '-t', '--template', 'template_ref', default='templates', help='Template flake reference (default: templates)'
)
@click.pass_context
def init(ctx, template_ref):
    """Create a flake in the current directory from a template.

    \b
    Examples:
      trix flake init                      # Use default template
      trix flake init -t templates#python  # Use python template
      trix flake init -t github:owner/repo#mytemplate
    """
    import shutil

    verbose = ctx.obj['verbose']
    cwd = Path.cwd()

    # Parse template reference
    if '#' in template_ref:
        flake_ref, template_name = template_ref.rsplit('#', 1)
    else:
        flake_ref = template_ref
        template_name = 'default'

    # Resolve "templates" shorthand to the official NixOS templates flake
    if flake_ref == 'templates':
        flake_ref = 'github:NixOS/templates'

    # Build the template path using nix flake prefetch to get the source
    if verbose:
        click.echo(f'+ Fetching template from {flake_ref}#{template_name}', err=True)

    # Use nix flake prefetch to get the flake source
    prefetch_cmd = [
        'nix',
        '--extra-experimental-features',
        'nix-command flakes',
        'flake',
        'prefetch',
        '--json',
        flake_ref,
    ]
    if verbose:
        click.echo(f'+ {" ".join(prefetch_cmd)}', err=True)

    result = subprocess.run(prefetch_cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        click.echo(f'Failed to fetch template flake: {result.stderr}', err=True)
        sys.exit(1)

    prefetch_info = json.loads(result.stdout)
    flake_store_path = prefetch_info.get('storePath')

    if not flake_store_path:
        click.echo('Could not determine flake store path', err=True)
        sys.exit(1)

    # Load the flake.nix to find the template
    flake_path = Path(flake_store_path)
    flake_nix_path = flake_path / 'flake.nix'

    if not flake_nix_path.exists():
        click.echo(f'No flake.nix found in {flake_store_path}', err=True)
        sys.exit(1)

    # Evaluate the template path from the flake
    # templates.NAME.path gives us the directory to copy
    template_attr = f'templates.{template_name}'

    nix_dir = get_nix_dir()
    system = get_system()

    # Check if template flake has a lock file
    lock_file = flake_path / 'flake.lock'
    if lock_file.exists():
        lock_expr = f'builtins.fromJSON (builtins.readFile {lock_file})'
    else:
        lock_expr = '{ nodes = { root = { inputs = {}; }; }; root = "root"; version = 7; }'

    # Use proper input resolution via inputs.nix (same as other flake evaluation)
    # For 'default', try both defaultTemplate and templates.default (nix supports both)
    eval_expr = f"""
    let
      flake = import {flake_nix_path};
      lock = {lock_expr};
      inputs = import {nix_dir}/inputs.nix {{
        inherit lock;
        flakeDirPath = {flake_path};
        system = "{system}";
      }};
      outputs = flake.outputs (inputs // {{ self = inputs.self // outputs; }});
      template =
        {"outputs.defaultTemplate or outputs." + template_attr if template_name == 'default' else "outputs." + template_attr};
    in {{
      path = toString template.path;
      description = template.description or "";
      welcomeText = template.welcomeText or "";
    }}
    """

    eval_cmd = ['nix-instantiate', '--eval', '--expr', eval_expr, '--json', '--strict', '--read-write-mode']
    if verbose:
        click.echo('+ nix-instantiate --eval ... (getting template info)', err=True)

    result = subprocess.run(eval_cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        click.echo(f"Template '{template_name}' not found in {flake_ref}", err=True)
        if verbose:
            click.echo(result.stderr, err=True)
        sys.exit(1)

    template_info = json.loads(result.stdout)
    template_path = Path(template_info['path'])
    welcome_text = template_info.get('welcomeText', '')

    if not template_path.exists():
        click.echo(f'Template path does not exist: {template_path}', err=True)
        sys.exit(1)

    # Copy files from template to current directory (don't overwrite)
    copied_files = []
    skipped_files = []

    for src_file in template_path.rglob('*'):
        if src_file.is_file():
            rel_path = src_file.relative_to(template_path)
            dest_file = cwd / rel_path

            if dest_file.exists():
                skipped_files.append(rel_path)
            else:
                dest_file.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(src_file, dest_file)
                # Make writable (store files are read-only)
                dest_file.chmod(dest_file.stat().st_mode | 0o200)
                copied_files.append(rel_path)
                if verbose:
                    click.echo(f'  wrote: {rel_path}')

    # Report results
    if copied_files:
        click.echo(f"Wrote {len(copied_files)} file(s) from template '{template_name}'")
    else:
        click.echo('No files were written (all files already exist)')

    if skipped_files and verbose:
        click.echo(f'Skipped {len(skipped_files)} existing file(s): {", ".join(str(f) for f in skipped_files)}')

    # Display welcome text if present
    if welcome_text:
        click.echo()
        click.echo(welcome_text)


@flake.command()
@click.argument('path')
@click.option(
    '-t', '--template', 'template_ref', default='templates', help='Template flake reference (default: templates)'
)
@click.pass_context
def new(ctx, path, template_ref):
    """Create a new directory with a flake from a template.

    \b
    Examples:
      trix flake new myproject                       # Use default template
      trix flake new myproject -t templates#python   # Use python template
      trix flake new myproject -t github:owner/repo#mytemplate
    """
    import shutil

    verbose = ctx.obj['verbose']
    target_dir = Path(path)

    if target_dir.exists():
        click.echo(f'Directory already exists: {target_dir}', err=True)
        sys.exit(1)

    # Create the directory
    target_dir.mkdir(parents=True)

    # Parse template reference
    if '#' in template_ref:
        flake_ref, template_name = template_ref.rsplit('#', 1)
    else:
        flake_ref = template_ref
        template_name = 'default'

    # Resolve "templates" shorthand to the official NixOS templates flake
    if flake_ref == 'templates':
        flake_ref = 'github:NixOS/templates'

    # Build the template path using nix flake prefetch to get the source
    if verbose:
        click.echo(f'+ Fetching template from {flake_ref}#{template_name}', err=True)

    # Use nix flake prefetch to get the flake source
    prefetch_cmd = [
        'nix',
        '--extra-experimental-features',
        'nix-command flakes',
        'flake',
        'prefetch',
        '--json',
        flake_ref,
    ]
    if verbose:
        click.echo(f'+ {" ".join(prefetch_cmd)}', err=True)

    result = subprocess.run(prefetch_cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        click.echo(f'Failed to fetch template flake: {result.stderr}', err=True)
        target_dir.rmdir()  # Clean up empty directory
        sys.exit(1)

    prefetch_info = json.loads(result.stdout)
    flake_store_path = prefetch_info.get('storePath')

    if not flake_store_path:
        click.echo('Could not determine flake store path', err=True)
        target_dir.rmdir()
        sys.exit(1)

    # Load the flake.nix to find the template
    flake_path = Path(flake_store_path)
    flake_nix_path = flake_path / 'flake.nix'

    if not flake_nix_path.exists():
        click.echo(f'No flake.nix found in {flake_store_path}', err=True)
        target_dir.rmdir()
        sys.exit(1)

    # Evaluate the template path from the flake
    template_attr = f'templates.{template_name}'

    nix_dir = get_nix_dir()
    system = get_system()

    # Check if template flake has a lock file
    lock_file = flake_path / 'flake.lock'
    if lock_file.exists():
        lock_expr = f'builtins.fromJSON (builtins.readFile {lock_file})'
    else:
        lock_expr = '{ nodes = { root = { inputs = {}; }; }; root = "root"; version = 7; }'

    # Use proper input resolution via inputs.nix (same as other flake evaluation)
    # For 'default', try both defaultTemplate and templates.default (nix supports both)
    eval_expr = f"""
    let
      flake = import {flake_nix_path};
      lock = {lock_expr};
      inputs = import {nix_dir}/inputs.nix {{
        inherit lock;
        flakeDirPath = {flake_path};
        system = "{system}";
      }};
      outputs = flake.outputs (inputs // {{ self = inputs.self // outputs; }});
      template =
        {"outputs.defaultTemplate or outputs." + template_attr if template_name == 'default' else "outputs." + template_attr};
    in {{
      path = toString template.path;
      description = template.description or "";
      welcomeText = template.welcomeText or "";
    }}
    """

    eval_cmd = ['nix-instantiate', '--eval', '--expr', eval_expr, '--json', '--strict', '--read-write-mode']
    if verbose:
        click.echo('+ nix-instantiate --eval ... (getting template info)', err=True)

    result = subprocess.run(eval_cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        click.echo(f"Template '{template_name}' not found in {flake_ref}", err=True)
        if verbose:
            click.echo(result.stderr, err=True)
        target_dir.rmdir()
        sys.exit(1)

    template_info = json.loads(result.stdout)
    template_path = Path(template_info['path'])
    welcome_text = template_info.get('welcomeText', '')

    if not template_path.exists():
        click.echo(f'Template path does not exist: {template_path}', err=True)
        target_dir.rmdir()
        sys.exit(1)

    # Copy files from template to target directory
    copied_files = []

    for src_file in template_path.rglob('*'):
        if src_file.is_file():
            rel_path = src_file.relative_to(template_path)
            dest_file = target_dir / rel_path

            dest_file.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src_file, dest_file)
            # Make writable (store files are read-only)
            dest_file.chmod(dest_file.stat().st_mode | 0o200)
            copied_files.append(rel_path)
            if verbose:
                click.echo(f'  wrote: {rel_path}')

    # Report results
    if copied_files:
        click.echo(f"Created flake in '{target_dir}' with {len(copied_files)} file(s)")
    else:
        click.echo(f"Created empty flake directory '{target_dir}'")

    # Display welcome text if present
    if welcome_text:
        click.echo()
        click.echo(welcome_text)
