"""CLI commands for managing flake registries."""

import sys

import click

from .registry import (
    add_registry_entry,
    list_all_registries,
    registry_entry_to_flake_ref,
    remove_registry_entry,
)


@click.group()
def registry():
    """Manage flake registries.

    Registry entries map short names (like 'nixpkgs') to flake references.
    Entries can point to local paths (handled natively by trix) or remote
    refs (passed through to nix).

    User registry entries are stored in ~/.config/nix/registry.json
    """
    pass


@registry.command('list')
@click.option('--no-global', is_flag=True, help="Don't fetch the global registry")
def list_cmd(no_global):
    """List all registry entries.

    Shows entries from user, system, and global registries.
    User entries override system entries, which override global entries.

    \b
    Examples:
      trix registry list
      trix registry list --no-global
    """
    entries = list_all_registries(use_global=not no_global)

    if not entries:
        click.echo('No registry entries found.')
        return

    # Group by source
    by_source = {'user': [], 'system': [], 'global': []}
    for name, source, entry in entries:
        by_source[source].append((name, entry))

    for source in ['user', 'system', 'global']:
        if by_source[source]:
            click.echo(f'\n{source.upper()} registry:')
            for name, entry in sorted(by_source[source]):
                ref = registry_entry_to_flake_ref(entry)
                entry_type = entry.get('type', 'unknown')
                if entry_type == 'path':
                    click.echo(f'  {name} -> {ref} (local)')
                else:
                    click.echo(f'  {name} -> {ref}')


@registry.command()
@click.argument('name')
@click.argument('target')
def add(name, target):
    """Add or update a registry entry.

    NAME is the short name to register (e.g., 'nixpkgs', 'myflake').
    TARGET is the flake reference to map it to.

    For local paths, trix handles builds natively without copying the
    entire flake to the store. For remote refs, trix passes through to nix.

    \b
    Examples:
      trix registry add nixpkgs ~/code/nixpkgs     # Local path
      trix registry add nixpkgs /nix/var/nixpkgs   # Absolute path
      trix registry add nixpkgs github:NixOS/nixpkgs/nixos-24.05  # Remote
      trix registry add myflake .                  # Current directory
    """
    add_registry_entry(name, target)

    # Show what was added
    from .registry import resolve_registry_name

    entry = resolve_registry_name(name, use_global=False)
    if entry:
        ref = registry_entry_to_flake_ref(entry)
        entry_type = entry.get('type', 'unknown')
        if entry_type == 'path':
            click.echo(f'Added: {name} -> {ref} (local, handled natively by trix)')
        else:
            click.echo(f'Added: {name} -> {ref} (remote, passthrough to nix)')


@registry.command()
@click.argument('name')
def remove(name):
    """Remove a registry entry.

    Only entries in the user registry can be removed.
    System and global registry entries cannot be removed (but can be
    overridden by adding a user entry with the same name).

    \b
    Examples:
      trix registry remove myflake
      trix registry remove nixpkgs  # Remove user override
    """
    if remove_registry_entry(name):
        click.echo(f'Removed: {name}')
    else:
        click.echo(f"Entry '{name}' not found in user registry.", err=True)
        sys.exit(1)
