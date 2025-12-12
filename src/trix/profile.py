"""Profile management for trix.

Compatible with nix profile's manifest.json format (version 3).
Supports both local flake packages (via flake-compat) and remote packages.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from .nix import _get_clean_env, get_store_dir, get_system, run_nix_build


def get_profile_dir() -> Path:
    """Get the profile directory (where profile-N-link symlinks live).

    Returns the parent directory of the profile symlink target.
    """
    nix_profile = Path(os.environ.get('NIX_PROFILE', Path.home() / '.nix-profile'))
    if nix_profile.is_symlink():
        target = Path(os.readlink(nix_profile))
        if target.is_absolute():
            return target.parent
        return (nix_profile.parent / target).parent
    return Path(f'/nix/var/nix/profiles/per-user/{os.environ.get("USER", "default")}')


def get_current_profile_path() -> Path | None:
    """Get the store path of the current profile generation."""
    nix_profile = Path(os.environ.get('NIX_PROFILE', Path.home() / '.nix-profile'))
    if nix_profile.exists():
        return Path(os.path.realpath(nix_profile))
    return None


def get_current_manifest() -> dict:
    """Read the current profile's manifest.json.

    Returns a dict with 'version' and 'elements' keys.
    """
    profile_path = get_current_profile_path()
    if profile_path:
        manifest_path = profile_path / 'manifest.json'
        if manifest_path.exists():
            with open(manifest_path) as f:
                return json.load(f)
    return {'version': 3, 'elements': {}}


def parse_generation_number(filename: str) -> int | None:
    """Parse generation number from a profile link filename.

    Args:
        filename: e.g., "profile-42-link"

    Returns:
        The generation number (e.g., 42), or None if parsing fails.
    """
    if not filename.startswith('profile-') or not filename.endswith('-link'):
        return None
    try:
        return int(filename.replace('profile-', '').replace('-link', ''))
    except ValueError:
        return None


def get_next_profile_number() -> int:
    """Get the next profile generation number."""
    profile_dir = get_profile_dir()
    max_num = 0
    try:
        for entry in profile_dir.iterdir():
            num = parse_generation_number(entry.name)
            if num is not None:
                max_num = max(max_num, num)
    except FileNotFoundError:
        pass
    return max_num + 1


def collect_package_paths(store_paths: list[str]) -> dict[str, list[str]]:
    """Collect all files/dirs from packages that need to be symlinked in the profile.

    Returns a dict mapping relative paths to list of target paths.
    Multiple packages may provide the same path (e.g., bin/), which requires merging.
    """
    entries = {}

    for store_path in store_paths:
        pkg_path = Path(store_path)
        if not pkg_path.exists():
            continue

        for item in pkg_path.iterdir():
            if item.name == 'manifest.json':
                continue
            rel_name = item.name
            if rel_name not in entries:
                entries[rel_name] = []
            entries[rel_name].append(str(item))

    return entries


def create_profile_store_path(manifest: dict, store_paths: list[str]) -> str:
    """Create a new profile store path with the given manifest and packages.

    Creates a temporary directory with:
    - manifest.json containing package metadata
    - Symlinks to package contents (merging directories when needed)

    Returns the new store path.
    """
    with tempfile.TemporaryDirectory() as tmpdir:
        profile_dir = Path(tmpdir) / 'profile'
        profile_dir.mkdir()

        # Write manifest.json
        with open(profile_dir / 'manifest.json', 'w') as f:
            json.dump(manifest, f, separators=(',', ':'))

        # Collect and create symlinks for package contents
        entries = collect_package_paths(store_paths)

        for name, targets in entries.items():
            if len(targets) == 1:
                # Single package provides this path - simple symlink
                (profile_dir / name).symlink_to(targets[0])
            else:
                # Multiple packages - merge directory contents
                merged_dir = profile_dir / name
                merged_dir.mkdir()

                for target in targets:
                    target_path = Path(target)
                    if target_path.is_dir():
                        for item in target_path.iterdir():
                            dest = merged_dir / item.name
                            if not dest.exists():
                                dest.symlink_to(item)
                    else:
                        dest = merged_dir / target_path.name
                        if not dest.exists():
                            dest.symlink_to(target)

        result = subprocess.run(
            ['nix-store', '--add', str(profile_dir)], capture_output=True, text=True, check=True, env=_get_clean_env()
        )
        return result.stdout.strip()


def switch_profile(new_store_path: str) -> None:
    """Switch to a new profile generation atomically.

    Creates profile-N-link pointing to new_store_path,
    then atomically renames to update the profile symlink.
    """
    profile_dir = get_profile_dir()
    next_num = get_next_profile_number()

    # Create new profile-N-link
    new_link = profile_dir / f'profile-{next_num}-link'
    new_link.symlink_to(new_store_path)

    # Atomically update profile symlink
    profile_link = profile_dir / 'profile'
    tmp_link = profile_dir / f'profile-{next_num}-tmp'
    tmp_link.symlink_to(f'profile-{next_num}-link')
    tmp_link.rename(profile_link)


def list_installed() -> list[dict]:
    """List installed packages from manifest.

    Returns:
        List of dicts with package info (name, storePaths, originalUrl, attrPath)
    """
    manifest = get_current_manifest()
    packages = []
    for name, element in manifest.get('elements', {}).items():
        if element.get('active', True):
            packages.append(
                {
                    'name': name,
                    'storePaths': element.get('storePaths', []),
                    'originalUrl': element.get('originalUrl', ''),
                    'attrPath': element.get('attrPath', ''),
                    'priority': element.get('priority', 5),
                }
            )
    return packages


def list_installed_names() -> list[str]:
    """List installed package names.

    Returns:
        List of package names
    """
    return [pkg['name'] for pkg in list_installed()]


def is_local_path(path: str) -> bool:
    """Check if a string looks like a local path."""
    return path in ('.', '') or path.startswith(('./', '/', '~', '../'))


def parse_installable_for_profile(installable: str) -> tuple[str, str, str]:
    """Parse an installable reference for profile operations.

    Returns (ref_part, attr, pkg_name) where:
    - ref_part: The flake reference part (e.g., ".", "/path/to/flake", "github:...")
    - attr: The attribute name (e.g., "hello", "default")
    - pkg_name: The package name for the manifest (uses directory name for default packages)

    Examples:
        ".#hello" -> (".", "hello", "hello")
        ".#" -> (".", "default", "<directory_name>")
        "." -> (".", "default", "<directory_name>")
        "github:NixOS/nixpkgs#hello" -> ("github:NixOS/nixpkgs", "hello", "hello")
    """
    if '#' in installable:
        ref_part, attr = installable.rsplit('#', 1)
        # Empty attr means default (e.g., ".#" -> ".#default")
        if not attr:
            attr = 'default'
    else:
        ref_part = installable
        attr = 'default'

    # For default packages from local flakes, use directory name (matching nix behavior)
    if attr == 'default' and is_local_path(ref_part):
        flake_path = Path(ref_part).resolve() if ref_part and ref_part != '.' else Path.cwd()
        pkg_name = flake_path.name
    else:
        pkg_name = attr

    return ref_part, attr, pkg_name


def install(
    installable: str,
    flake_dir: Path | None = None,
    attr: str | None = None,
    store_path: str | None = None,
    verbose: bool = False,
) -> bool:
    """Install a package to the profile.

    Can be called in two ways:
    1. With installable string (e.g., ".#hello", "github:NixOS/nixpkgs#hello")
    2. With pre-built store_path and metadata (flake_dir, attr)

    Args:
        installable: Flake reference (e.g., ".#hello")
        flake_dir: Path to flake directory (if already parsed)
        attr: Attribute name (if already parsed)
        store_path: Pre-built store path (skips building)
        verbose: Print progress

    Returns:
        True on success
    """
    # If we have a pre-built store path, use it directly
    if store_path and flake_dir and attr:
        pkg_name = attr.split('.')[-1] if '.' in attr else attr
        system = get_system()
        original_url = f'path:{flake_dir}'
        attr_path = f'packages.{system}.{attr}' if not attr.startswith('packages.') else attr
    else:
        # Parse the installable
        ref_part, attr, pkg_name = parse_installable_for_profile(installable)

        # Check if it's a local path
        if is_local_path(ref_part):
            resolved_path = Path(ref_part).resolve() if ref_part and ref_part != '.' else Path.cwd()

            # Check if it's a store path (already built derivation)
            store_dir = get_store_dir()
            if str(resolved_path).startswith(f'{store_dir}/'):
                store_path = str(resolved_path)
                # Extract package name from store path (e.g., /nix/store/xxx-hello-1.0 -> hello)
                store_name = resolved_path.name
                # Remove hash prefix (32 chars + dash)
                if len(store_name) > 33 and store_name[32] == '-':
                    name_version = store_name[33:]
                    # Try to extract just the name (before version)
                    match = re.match(r'^(.+?)-\d', name_version)
                    pkg_name = match.group(1) if match else name_version
                else:
                    pkg_name = store_name
                original_url = f'path:{resolved_path}'
                attr_path = ''  # No attr path for direct store paths
            elif (resolved_path / 'flake.nix').exists():
                # Local flake directory
                flake_dir = resolved_path
                system = get_system()
                original_url = f'path:{flake_dir}'
                attr_path = f'packages.{system}.{attr}'

                if verbose:
                    print(f'Building {installable}...', file=sys.stderr)

                # Build using our nix wrapper
                store_path = run_nix_build(
                    flake_dir=flake_dir,
                    attr=attr_path,
                    out_link=None,
                    verbose=verbose,
                    capture_output=True,
                )

                if not store_path:
                    print('Build produced no output', file=sys.stderr)
                    return False
            else:
                print(f"error: '{resolved_path}' does not contain a flake.nix", file=sys.stderr)
                return False
        else:
            # Remote flake - delegate to nix profile install
            if verbose:
                print(f'Installing {installable} via nix profile...', file=sys.stderr)
            cmd = ['nix', '--extra-experimental-features', 'nix-command flakes', 'profile', 'install', installable]
            result = subprocess.run(cmd, env=_get_clean_env())
            return result.returncode == 0

    # Update manifest with new package
    manifest = get_current_manifest()

    if pkg_name in manifest['elements']:
        existing = manifest['elements'][pkg_name].get('storePaths', [])
        if existing and existing[0] == store_path:
            if verbose:
                print(f'{pkg_name} is already installed at this version', file=sys.stderr)
            return True
        if verbose:
            print(f'Replacing existing {pkg_name}', file=sys.stderr)

    manifest['elements'][pkg_name] = {
        'active': True,
        'priority': 5,
        'storePaths': [store_path],
        'originalUrl': original_url,
        'url': original_url,
        'attrPath': attr_path,
    }

    # Collect all store paths for the new profile
    all_store_paths = []
    for element in manifest['elements'].values():
        all_store_paths.extend(element.get('storePaths', []))

    if verbose:
        print('Creating new profile generation...', file=sys.stderr)

    try:
        new_profile_path = create_profile_store_path(manifest, all_store_paths)
        if verbose:
            print(f'Created {new_profile_path}', file=sys.stderr)
        switch_profile(new_profile_path)
    except subprocess.CalledProcessError as e:
        print(f'Failed to create profile: {e.stderr}', file=sys.stderr)
        return False

    return True


def remove(name: str, verbose: bool = False) -> bool:
    """Remove a package from the profile.

    Args:
        name: Package name
        verbose: Print progress

    Returns:
        True on success
    """
    # Check if user is trying to use a numeric index (no longer supported)
    if name.isdigit():
        print(f"error: 'trix profile' does not support indices ('{name}')", file=sys.stderr)
        return False

    manifest = get_current_manifest()

    if name not in manifest.get('elements', {}):
        # Try partial match
        matches = [n for n in manifest.get('elements', {}) if name in n]
        if len(matches) == 1:
            name = matches[0]
        elif len(matches) > 1:
            print(f"Ambiguous package name '{name}', matches: {', '.join(matches)}", file=sys.stderr)
            return False
        else:
            print(f"Package '{name}' not found in profile", file=sys.stderr)
            return False

    if verbose:
        print(f'Removing {name}...', file=sys.stderr)

    del manifest['elements'][name]

    # Collect remaining store paths
    all_store_paths = []
    for element in manifest['elements'].values():
        all_store_paths.extend(element.get('storePaths', []))

    if verbose:
        print('Creating new profile generation...', file=sys.stderr)

    try:
        new_profile_path = create_profile_store_path(manifest, all_store_paths)
        switch_profile(new_profile_path)
    except subprocess.CalledProcessError as e:
        print(f'Failed to create profile: {e.stderr}', file=sys.stderr)
        return False

    return True


def upgrade(name: str | None = None, verbose: bool = False) -> tuple[int, int]:
    """Upgrade local packages in profile.

    Args:
        name: Package name to upgrade (None for all local packages)
        verbose: Print progress

    Returns:
        Tuple of (upgraded_count, skipped_count)
    """
    manifest = get_current_manifest()

    if not manifest.get('elements'):
        if verbose:
            print('No packages in profile', file=sys.stderr)
        return (0, 0)

    # Find local packages (those with path: URLs)
    local_packages = {}
    for pkg_name, element in manifest['elements'].items():
        original_url = element.get('originalUrl', '')
        if original_url.startswith('path:'):
            if name is None or name == pkg_name or name in pkg_name:
                local_packages[pkg_name] = element

    if not local_packages:
        if verbose:
            print('No local packages to upgrade', file=sys.stderr)
        return (0, 0)

    upgraded = 0
    skipped = 0
    modified = False

    for pkg_name, element in local_packages.items():
        original_url = element.get('originalUrl', '')
        attr_path = element.get('attrPath', '')
        old_store_paths = element.get('storePaths', [])
        old_store_path = old_store_paths[0] if old_store_paths else ''

        # Extract flake path from URL
        flake_path = original_url.removeprefix('path:')
        flake_path = flake_path.split('?')[0]

        if verbose:
            print(f'Upgrading {pkg_name} from {flake_path}...', file=sys.stderr)

        if not Path(flake_path).is_dir():
            print(f'Warning: {flake_path} no longer exists, skipping', file=sys.stderr)
            skipped += 1
            continue

        # Build the package
        try:
            new_store_path = run_nix_build(
                flake_dir=Path(flake_path),
                attr=attr_path,
                out_link=None,
                verbose=verbose,
                capture_output=True,
            )
        except Exception as e:
            print(f'Failed to build {pkg_name}: {e}', file=sys.stderr)
            skipped += 1
            continue

        if new_store_path == old_store_path:
            if verbose:
                print(f'{pkg_name} is up to date', file=sys.stderr)
            skipped += 1
            continue

        if verbose:
            print(f'Built {new_store_path}', file=sys.stderr)

        manifest['elements'][pkg_name]['storePaths'] = [new_store_path]
        modified = True
        upgraded += 1

    if modified:
        all_store_paths = []
        for element in manifest['elements'].values():
            all_store_paths.extend(element.get('storePaths', []))

        if verbose:
            print('Creating new profile generation...', file=sys.stderr)

        try:
            new_profile_path = create_profile_store_path(manifest, all_store_paths)
            if verbose:
                print(f'Created {new_profile_path}', file=sys.stderr)
            switch_profile(new_profile_path)
        except subprocess.CalledProcessError as e:
            print(f'Failed to create profile: {e.stderr}', file=sys.stderr)

    return (upgraded, skipped)
