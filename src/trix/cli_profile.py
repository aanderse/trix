"""CLI commands for profile management."""

import sys
from pathlib import Path

import click

from . import profile as prof


@click.group()
@click.pass_context
def profile(ctx):
    """Manage installed packages (nix profile compatible).

    \b
    Examples:
      trix profile list
      trix profile add .#hello
      trix profile remove hello
      trix profile upgrade
    """
    pass


def _bold(text: str) -> str:
    """Wrap text in ANSI bold codes."""
    return f'\033[1m{text}\033[0m'


def _green_bold(text: str) -> str:
    """Wrap text in ANSI green+bold codes (for current generation)."""
    return f'\033[32;1m{text}\033[0m'


@profile.command('list')
@click.option('--json', 'output_json', is_flag=True, help='Output as JSON')
@click.pass_context
def profile_list(ctx, output_json):
    """List installed packages."""
    packages = prof.list_installed()

    if not packages:
        if not output_json:
            click.echo('No packages installed.')
        else:
            click.echo('[]')
        return

    if output_json:
        import json

        click.echo(json.dumps(packages, indent=2))
    else:
        for i, pkg in enumerate(packages):
            if i > 0:
                click.echo()  # Blank line between entries

            name = pkg['name']
            store_paths = pkg.get('storePaths', [])
            original_url = pkg.get('originalUrl', '')
            attr_path = pkg.get('attrPath', '')

            # Match nix profile list format exactly
            click.echo(f'Name:               {_bold(name)}')

            # Flake attribute (the attr path like packages.x86_64-linux.hello)
            if attr_path:
                click.echo(f'Flake attribute:    {attr_path}')

            # Original flake URL
            if original_url:
                click.echo(f'Original flake URL: {original_url}')

            # Store paths
            if store_paths:
                click.echo(f'Store paths:        {store_paths[0]}')
                for path in store_paths[1:]:
                    click.echo(f'                    {path}')


@profile.command('add')
@click.argument('installables', nargs=-1, required=True)
@click.pass_context
def profile_add(ctx, installables):
    """Add packages to the profile.

    Supports local flake references and remote flakes.

    \b
    Examples:
      trix profile add .#hello                    # From local flake.nix
      trix profile add /path/to/flake#pkg
      trix profile add github:NixOS/nixpkgs#hello # From remote flake
    """
    verbose = ctx.obj.get('verbose', False)

    for installable in installables:
        if prof.install(installable, verbose=verbose):
            # Extract package name for display
            _, _, pkg_name = prof.parse_installable_for_profile(installable)
            click.echo(f'Added {pkg_name}')
        else:
            click.echo(f'Failed to add {installable}', err=True)
            sys.exit(1)


# Alias for backwards compatibility
@profile.command('install', hidden=True)
@click.argument('installables', nargs=-1, required=True)
@click.pass_context
def profile_install(ctx, installables):
    """Alias for 'add' (for backwards compatibility)."""
    ctx.invoke(profile_add, installables=installables)


@profile.command('remove')
@click.argument('names', nargs=-1, required=True)
@click.pass_context
def profile_remove(ctx, names):
    """Remove packages from the profile.

    \b
    Examples:
      trix profile remove hello
      trix profile remove hello cowsay
    """
    verbose = ctx.obj.get('verbose', False)

    for name in names:
        if prof.remove(name, verbose=verbose):
            click.echo(f'Removed {name}')
        else:
            sys.exit(1)


@profile.command('upgrade')
@click.argument('name', required=False)
@click.pass_context
def profile_upgrade(ctx, name):
    """Upgrade local packages in the profile.

    Rebuilds packages installed from local paths (path:...) and updates
    the profile if the output changed.

    \b
    Examples:
      trix profile upgrade         # Upgrade all local packages
      trix profile upgrade hello   # Upgrade only 'hello'
    """
    verbose = ctx.obj.get('verbose', False)

    upgraded, skipped = prof.upgrade(name, verbose=verbose)

    if upgraded > 0:
        click.echo(f'Upgraded {upgraded} package(s)')
    elif skipped > 0:
        click.echo(f'All {skipped} package(s) up to date')
    else:
        click.echo('No local packages to upgrade')


def _get_generation_manifest(target: Path) -> dict:
    """Read manifest.json from a profile generation."""
    manifest_path = target / 'manifest.json'
    if manifest_path.exists():
        import json

        with open(manifest_path) as f:
            return json.load(f)
    return {'version': 3, 'elements': {}}


def _extract_version(store_path: str) -> str:
    """Extract version from a store path like /nix/store/xxx-name-1.2.3."""
    # Store paths are like /nix/store/hash-name-version
    # Try to extract version from the end
    import re

    basename = Path(store_path).name
    # Remove the hash prefix (32 chars + dash)
    if len(basename) > 33 and basename[32] == '-':
        name_version = basename[33:]
        # Try to find version at end (after last dash followed by digit)
        match = re.search(r'-(\d+\.\d+.*?)$', name_version)
        if match:
            return match.group(1)
        # If no clear version, use the whole name-version part
        return name_version
    return store_path


def _get_package_versions(manifest: dict) -> dict[str, str]:
    """Get package name -> version mapping from manifest."""
    versions = {}
    for name, element in manifest.get('elements', {}).items():
        if element.get('active', True):
            store_paths = element.get('storePaths', [])
            if store_paths:
                versions[name] = _extract_version(store_paths[0])
            else:
                versions[name] = 'unknown'
    return versions


@profile.command('history')
@click.pass_context
def profile_history(ctx):
    """Show profile generation history.

    \b
    Examples:
      trix profile history
    """
    from datetime import datetime

    profile_dir = prof.get_profile_dir()

    generations = []
    try:
        for entry in profile_dir.iterdir():
            num = prof.parse_generation_number(entry.name)
            if num is not None:
                try:
                    target = entry.resolve()
                    # Get modification time from the symlink itself
                    mtime = entry.lstat().st_mtime
                    generations.append((num, entry, target, mtime))
                except OSError:
                    pass
    except FileNotFoundError:
        click.echo('No profile generations found')
        return

    if not generations:
        click.echo('No profile generations found')
        return

    generations.sort(key=lambda x: x[0])

    prev_versions = {}

    for i, (num, _link, target, mtime) in enumerate(generations):
        # Format date
        date_str = datetime.fromtimestamp(mtime).strftime('%Y-%m-%d')

        # Build header - highlight current generation (last one) in green
        is_current = i == len(generations) - 1
        version_str = _green_bold(str(num)) if is_current else _bold(str(num))

        if i == 0:
            header = f'Version {version_str} ({date_str}):'
        else:
            prev_num = generations[i - 1][0]
            header = f'Version {version_str} ({date_str}) <- {prev_num}:'

        click.echo(header)

        # Get current manifest and versions
        manifest = _get_generation_manifest(target)
        curr_versions = _get_package_versions(manifest)

        # Find changes
        all_packages = set(prev_versions.keys()) | set(curr_versions.keys())
        changes = []

        for pkg in sorted(all_packages):
            old_ver = prev_versions.get(pkg)
            new_ver = curr_versions.get(pkg)

            if old_ver is None and new_ver is not None:
                # Added
                changes.append(f'  {pkg}: ∅ -> {new_ver}')
            elif old_ver is not None and new_ver is None:
                # Removed
                changes.append(f'  {pkg}: {old_ver} -> ∅')
            elif old_ver != new_ver:
                # Changed
                changes.append(f'  {pkg}: {old_ver} -> {new_ver}')

        if changes:
            for change in changes:
                click.echo(change)
        else:
            click.echo('  No changes.')

        click.echo()  # Blank line between versions

        prev_versions = curr_versions


@profile.command('rollback')
@click.pass_context
def profile_rollback(ctx):
    """Roll back to the previous profile generation.

    \b
    Examples:
      trix profile rollback
    """
    profile_dir = prof.get_profile_dir()

    generations = []
    try:
        for entry in profile_dir.iterdir():
            num = prof.parse_generation_number(entry.name)
            if num is not None:
                try:
                    target = entry.resolve()
                    generations.append((num, entry, target))
                except OSError:
                    pass
    except FileNotFoundError:
        click.echo('No profile generations found', err=True)
        sys.exit(1)

    if len(generations) < 2:
        click.echo('No previous generation to roll back to', err=True)
        sys.exit(1)

    generations.sort(key=lambda x: x[0])
    current = prof.get_current_profile_path()

    # Find current generation index
    current_idx = None
    for i, (_num, _link, target) in enumerate(generations):
        if current and target == current:
            current_idx = i
            break

    if current_idx is None or current_idx == 0:
        click.echo('No previous generation to roll back to', err=True)
        sys.exit(1)

    # Switch to previous generation
    prev_num, prev_link, prev_target = generations[current_idx - 1]

    profile_link = profile_dir / 'profile'
    next_num = prof.get_next_profile_number()
    tmp_link = profile_dir / f'profile-{next_num}-tmp'

    # Create new link pointing to previous generation's target
    new_link = profile_dir / f'profile-{next_num}-link'
    new_link.symlink_to(str(prev_target))

    # Atomically switch
    tmp_link.symlink_to(f'profile-{next_num}-link')
    tmp_link.rename(profile_link)

    click.echo(f'Rolled back to generation {prev_num}')


def _parse_older_than(value: str) -> int:
    """Parse --older-than value like '30d' to seconds."""
    import re

    match = re.match(r'^(\d+)d$', value)
    if not match:
        raise click.BadParameter(f"Invalid format '{value}', expected Nd (e.g., 30d)")
    days = int(match.group(1))
    return days * 24 * 60 * 60


@profile.command('wipe-history')
@click.option('--older-than', 'older_than', metavar='AGE', help='Only delete versions older than AGE (e.g., 30d)')
@click.option('--dry-run', is_flag=True, help='Show what would be deleted without deleting')
@click.pass_context
def profile_wipe_history(ctx, older_than, dry_run):
    """Delete non-current versions of the profile.

    By default, all non-current versions are deleted. With --older-than Nd,
    only versions older than N days are deleted.

    \b
    Examples:
      trix profile wipe-history                  # Delete all old versions
      trix profile wipe-history --older-than 30d # Only versions older than 30 days
      trix profile wipe-history --dry-run        # Show what would be deleted
    """
    import time

    profile_dir = prof.get_profile_dir()
    current_path = prof.get_current_profile_path()

    # Parse --older-than if provided
    max_age_seconds = None
    if older_than:
        max_age_seconds = _parse_older_than(older_than)

    now = time.time()
    to_delete = []

    try:
        for entry in profile_dir.iterdir():
            num = prof.parse_generation_number(entry.name)
            if num is not None:
                try:
                    target = entry.resolve()
                    # Skip current generation
                    if current_path and target == current_path:
                        continue

                    # Check age if --older-than specified
                    if max_age_seconds is not None:
                        mtime = entry.lstat().st_mtime
                        age = now - mtime
                        if age < max_age_seconds:
                            continue

                    to_delete.append((num, entry))
                except OSError:
                    pass
    except FileNotFoundError:
        click.echo('No profile generations found')
        return

    if not to_delete:
        click.echo('No profile versions to delete')
        return

    # Sort by generation number
    to_delete.sort(key=lambda x: x[0])

    for num, entry in to_delete:
        if dry_run:
            click.echo(f'would remove profile version {num}')
        else:
            click.echo(f'removing profile version {num}')
            entry.unlink()


def _get_closure(store_path: Path) -> set[str]:
    """Get the closure of a store path."""
    import subprocess

    result = subprocess.run(
        ['nix-store', '-qR', str(store_path)],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return set()
    return set(result.stdout.strip().split('\n')) if result.stdout.strip() else set()


def _get_store_path_size(store_path: str) -> int:
    """Get the size of a store path in bytes."""
    import subprocess

    result = subprocess.run(
        ['nix-store', '-q', '--size', store_path],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return 0
    try:
        return int(result.stdout.strip())
    except ValueError:
        return 0


def _parse_store_path_name(store_path: str) -> tuple[str, str]:
    """Parse a store path into (name, version).

    Store paths follow the convention: /nix/store/<32-char-hash>-<name>-<version>
    e.g., /nix/store/ld5p14p1a5k2zin7aw90na5mv71k311m-glibc-2.40-66

    We parse this heuristically because when comparing closures (from nix-store -qR),
    we only have store paths - not evaluated package metadata. This lets us detect
    version changes like "glibc: 2.39 -> 2.40" by matching package names.

    Returns (name, version) or (name, "") if no version found.
    """
    import re

    basename = Path(store_path).name
    # Remove the hash prefix (32 chars + dash)
    if len(basename) > 33 and basename[32] == '-':
        name_version = basename[33:]
        # Try to split name-version
        # Version typically starts with a digit after the last dash
        match = re.match(r'^(.+?)-(\d.*)$', name_version)
        if match:
            return match.group(1), match.group(2)
        return name_version, ''
    return basename, ''


def _format_size(size_bytes: int) -> str:
    """Format bytes as human-readable size."""
    if abs(size_bytes) < 1024:
        return f'{size_bytes} B'
    elif abs(size_bytes) < 1024 * 1024:
        return f'{size_bytes / 1024:.1f} KiB'
    elif abs(size_bytes) < 1024 * 1024 * 1024:
        return f'{size_bytes / (1024 * 1024):.1f} MiB'
    else:
        return f'{size_bytes / (1024 * 1024 * 1024):.1f} GiB'


def _red_bold(text: str) -> str:
    """Wrap text in ANSI red+bold codes (for size increases)."""
    return f'\033[31;1m{text}\033[0m'


@profile.command('diff-closures')
@click.pass_context
def profile_diff_closures(ctx):
    """Show closure difference between profile versions.

    Shows what packages changed between consecutive profile versions,
    including version changes and size differences.

    \b
    Examples:
      trix profile diff-closures
    """
    profile_dir = prof.get_profile_dir()

    generations = []
    try:
        for entry in profile_dir.iterdir():
            num = prof.parse_generation_number(entry.name)
            if num is not None:
                try:
                    target = entry.resolve()
                    generations.append((num, entry, target))
                except OSError:
                    pass
    except FileNotFoundError:
        click.echo('No profile generations found')
        return

    if len(generations) < 2:
        click.echo('Need at least 2 generations to show differences')
        return

    generations.sort(key=lambda x: x[0])

    # Compare consecutive generations
    for i in range(1, len(generations)):
        prev_num, _, prev_target = generations[i - 1]
        curr_num, _, curr_target = generations[i]

        # Get closures
        prev_closure = _get_closure(prev_target)
        curr_closure = _get_closure(curr_target)

        # Group by package name to find version changes
        prev_packages = {}
        for path in prev_closure:
            name, version = _parse_store_path_name(path)
            prev_packages[name] = (version, path)

        curr_packages = {}
        for path in curr_closure:
            name, version = _parse_store_path_name(path)
            curr_packages[name] = (version, path)

        changes = []

        # Find version changes and pure additions/removals
        all_names = set(prev_packages.keys()) | set(curr_packages.keys())
        for name in sorted(all_names):
            # Skip internal "profile" entry
            if name == 'profile':
                continue

            prev_info = prev_packages.get(name)
            curr_info = curr_packages.get(name)

            if prev_info and curr_info:
                prev_ver, prev_path = prev_info
                curr_ver, curr_path = curr_info
                if prev_path != curr_path:
                    # Version changed
                    prev_size = _get_store_path_size(prev_path)
                    curr_size = _get_store_path_size(curr_path)
                    size_diff = curr_size - prev_size
                    size_str = (
                        _red_bold(f'+{_format_size(size_diff)}')
                        if size_diff > 0
                        else f'-{_format_size(abs(size_diff))}'
                    )
                    if prev_ver and curr_ver and prev_ver != curr_ver:
                        changes.append(f'  {name}: {prev_ver} → {curr_ver}, {size_str}')
                    else:
                        changes.append(f'  {name}: {size_str}')
            elif curr_info:
                # Added
                curr_ver, curr_path = curr_info
                size = _get_store_path_size(curr_path)
                size_str = _red_bold(f'+{_format_size(size)}')
                if curr_ver:
                    changes.append(f'  {name}: ∅ → {curr_ver}, {size_str}')
                else:
                    changes.append(f'  {name}: ∅ → ?, {size_str}')
            elif prev_info:
                # Removed
                prev_ver, prev_path = prev_info
                size = _get_store_path_size(prev_path)
                if prev_ver:
                    changes.append(f'  {name}: {prev_ver} → ∅, -{_format_size(size)}')
                else:
                    changes.append(f'  {name}: ? → ∅, -{_format_size(size)}')

        if changes:
            click.echo(f'Version {prev_num} → {curr_num}:')
            for change in changes:
                click.echo(change)
            click.echo()
