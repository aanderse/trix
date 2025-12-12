"""Flake handling - parsing, URL resolution, lock management."""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .nix import _get_clean_env


@dataclass
class ResolvedInstallable:
    """Result of resolving an installable reference.

    Either local (flake_dir is set) or remote (flake_ref is set).
    """

    is_local: bool
    attr_part: str
    flake_dir: Path | None = None  # For local flakes
    flake_ref: str | None = None  # For remote refs (e.g., "github:NixOS/nixpkgs")


@dataclass
class FlakeInput:
    """Parsed flake input."""

    name: str
    type: str  # github, git, path, etc.
    owner: str | None = None
    repo: str | None = None
    ref: str | None = None
    path: str | None = None
    url: str | None = None


def parse_flake_url(url: str) -> dict:
    """Parse a flake URL into structured components.

    Examples:
        github:NixOS/nixpkgs -> {type: github, owner: NixOS, repo: nixpkgs}
        github:NixOS/nixpkgs/nixos-unstable -> {..., ref: nixos-unstable}
        github:NixOS/nixpkgs?ref=nixos-unstable -> {..., ref: nixos-unstable}
        path:./local -> {type: path, path: ./local}
        git+https://... -> {type: git, url: https://...}
    """
    # Handle query parameters
    query_params = {}
    if '?' in url:
        url, query = url.split('?', 1)
        for part in query.split('&'):
            if '=' in part:
                k, v = part.split('=', 1)
                query_params[k] = v

    # Parse by type
    if url.startswith('github:'):
        parts = url[7:].split('/')
        result = {'type': 'github', 'owner': parts[0], 'repo': parts[1]}
        if len(parts) > 2:
            result['ref'] = parts[2]
        if 'ref' in query_params:
            result['ref'] = query_params['ref']
        if 'rev' in query_params:
            result['rev'] = query_params['rev']
        return result

    elif url.startswith('git+'):
        # git+https://... or git+ssh://...
        actual_url = url[4:]
        result = {'type': 'git', 'url': actual_url}
        if 'ref' in query_params:
            result['ref'] = query_params['ref']
        if 'rev' in query_params:
            result['rev'] = query_params['rev']
        return result

    elif url.startswith('path:'):
        return {'type': 'path', 'path': url[5:]}

    elif url.startswith('/') or url.startswith('./') or url.startswith('../'):
        return {'type': 'path', 'path': url}

    else:
        # Unknown format, store as-is
        return {'type': 'unknown', 'url': url}


class UnsupportedFlakeFeature(Exception):
    """Raised when a flake uses features not supported by trix."""

    pass


def get_flake_inputs(flake_dir: Path) -> dict[str, dict]:
    """Extract inputs from flake.nix by evaluating with nix-instantiate.

    Returns a dict mapping input names to their specs. Each spec has:
    - type, owner, repo, ref, etc. for regular inputs
    - follows: dict mapping nested input names to follow paths for follows
    """
    # Extract all input info in a single nix-instantiate call
    expr = f"""
    let
      flake = import {flake_dir}/flake.nix;
      inputs = flake.inputs or {{}};

      # Extract info for a single input
      getInputInfo = name:
        let
          input = inputs.${{name}};
          # Handle both attrset inputs and string shorthand
          inputAttrs = if builtins.isAttrs input then input else {{ url = input; }};
        in {{
          inherit name;
          url = inputAttrs.url or null;
          follows = inputAttrs.follows or null;
          flake = inputAttrs.flake or true;
          # Get nested input follows (inputs.foo.inputs.bar.follows)
          nestedFollows =
            if inputAttrs ? inputs then
              builtins.listToAttrs (
                builtins.filter (x: x.value != null) (
                  map (nestedName: {{
                    name = nestedName;
                    value = inputAttrs.inputs.${{nestedName}}.follows or null;
                  }}) (builtins.attrNames inputAttrs.inputs)
                )
              )
            else {{}};
        }};

    in map getInputInfo (builtins.attrNames inputs)
    """

    result = subprocess.run(
        ['nix-instantiate', '--eval', '--expr', expr, '--json', '--strict'],
        capture_output=True,
        text=True,
        env=_get_clean_env(),
    )

    if result.returncode != 0:
        return {}

    raw_inputs = json.loads(result.stdout)
    if not raw_inputs:
        return {}

    # Convert raw data to parsed format
    parsed = {}
    for raw in raw_inputs:
        name = raw['name']

        # Check for root-level follows first
        if raw['follows'] is not None:
            follows_value = raw['follows']
            if '/' in follows_value:
                parsed[name] = {'type': 'follows', 'follows': follows_value.split('/')}
            else:
                parsed[name] = {'type': 'follows', 'follows': [follows_value]}
            continue

        # Regular input with URL
        url = raw['url']
        if url:
            parsed[name] = parse_flake_url(url)

            # Check for flake = false
            if not raw['flake']:
                parsed[name]['flake'] = False

            # Add nested follows if present
            if raw['nestedFollows']:
                follows = {}
                for nested_name, follows_value in raw['nestedFollows'].items():
                    if '/' in follows_value:
                        follows[nested_name] = follows_value.split('/')
                    else:
                        follows[nested_name] = [follows_value]
                parsed[name]['follows'] = follows

    return parsed


def get_flake_description(flake_dir: Path) -> str | None:
    """Extract description from flake.nix."""
    expr = f'(import {flake_dir}/flake.nix).description or null'
    result = subprocess.run(
        ['nix-instantiate', '--eval', '--expr', expr, '--json'],
        capture_output=True,
        text=True,
        env=_get_clean_env(),
    )

    if result.returncode == 0:
        desc = json.loads(result.stdout)
        return desc if desc else None
    return None


def get_nix_config(flake_dir: Path, warn_unsupported: bool = True) -> dict:
    """Extract nixConfig from flake.nix.

    Returns a dict with nixConfig options. Supported options:
    - bash-prompt: Custom shell prompt
    - bash-prompt-prefix: Prefix for shell prompt
    - bash-prompt-suffix: Suffix for shell prompt

    If warn_unsupported is True, prints a warning for any unsupported options.
    """
    import sys

    supported_options = {'bash-prompt', 'bash-prompt-prefix', 'bash-prompt-suffix'}

    # First, get all nixConfig attribute names to check for unsupported options
    expr_all = f'builtins.attrNames ((import {flake_dir}/flake.nix).nixConfig or {{}})'
    result_all = subprocess.run(
        ['nix-instantiate', '--eval', '--expr', expr_all, '--json'],
        capture_output=True,
        text=True,
        env=_get_clean_env(),
    )

    if result_all.returncode == 0:
        all_options = set(json.loads(result_all.stdout))
        unsupported = all_options - supported_options
        if unsupported and warn_unsupported:
            for opt in sorted(unsupported):
                print(f'warning: nixConfig.{opt} is not supported by trix', file=sys.stderr)

    # Now get the supported options
    expr = f"""
    let
      flake = import {flake_dir}/flake.nix;
      cfg = flake.nixConfig or {{}};
    in {{
      bash-prompt = cfg.bash-prompt or null;
      bash-prompt-prefix = cfg.bash-prompt-prefix or null;
      bash-prompt-suffix = cfg.bash-prompt-suffix or null;
    }}
    """
    result = subprocess.run(
        ['nix-instantiate', '--eval', '--expr', expr, '--json', '--strict'],
        capture_output=True,
        text=True,
        env=_get_clean_env(),
    )

    if result.returncode == 0:
        config = json.loads(result.stdout)
        # Filter out null values
        return {k: v for k, v in config.items() if v is not None}
    return {}


def parse_installable(installable: str) -> tuple[Path, str]:
    """Parse an installable reference to (flake_dir, attr_part).

    This does NOT resolve the system - use resolve_attr_path() for that.

    Args:
        installable: e.g., ".", ".#hello", ".#devShells.myshell"

    Returns:
        (flake_dir, attr_part) where attr_part is the raw attribute (e.g., "hello")
    """
    # Parse installable
    if '#' in installable:
        path_part, attr_part = installable.split('#', 1)
    else:
        path_part = installable
        attr_part = 'default'

    # Resolve flake directory
    if path_part == '' or path_part == '.':
        flake_dir = Path.cwd()
    else:
        flake_dir = Path(path_part).resolve()

    return flake_dir, attr_part


def resolve_installable(installable: str) -> ResolvedInstallable:
    """Resolve an installable reference, handling registry lookups.

    This function determines whether an installable is:
    1. A local flake (path-based) - handled natively by trix
    2. A remote flake (github:, git+, etc.) - passed through to nix
    3. A registry name (nixpkgs, home-manager) - resolved via registry

    Registry names are looked up in:
    - User registry (~/.config/nix/registry.json)
    - System registry (/etc/nix/registry.json)
    - Global registry (https://channels.nixos.org/flake-registry.json)

    If a registry entry resolves to a local path, it's handled natively.
    If it resolves to a remote ref, it's passed through to nix.

    Args:
        installable: e.g., ".", ".#hello", "nixpkgs#hello", "github:NixOS/nixpkgs#hello"

    Returns:
        ResolvedInstallable with either flake_dir (local) or flake_ref (remote)
    """
    from .registry import is_registry_name, registry_entry_to_flake_ref, resolve_registry_name

    # Parse the installable to separate path/ref part from attribute
    if '#' in installable:
        ref_part, attr_part = installable.split('#', 1)
    else:
        ref_part = installable
        attr_part = 'default'

    # Case 1: Empty or current directory
    if ref_part == '' or ref_part == '.':
        return ResolvedInstallable(
            is_local=True,
            attr_part=attr_part,
            flake_dir=Path.cwd(),
        )

    # Case 2: Explicit path (starts with /, ./, ../, ~, or path:)
    if ref_part.startswith(('/', './', '../', '~')) or ref_part.startswith('path:'):
        if ref_part.startswith('path:'):
            path = ref_part[5:]
        else:
            path = ref_part
        return ResolvedInstallable(
            is_local=True,
            attr_part=attr_part,
            flake_dir=Path(path).expanduser().resolve(),
        )

    # Case 3: Full flake reference (github:, git+, etc.)
    if ':' in ref_part:
        return ResolvedInstallable(
            is_local=False,
            attr_part=attr_part,
            flake_ref=ref_part,
        )

    # Case 4: Registry name (e.g., "nixpkgs", "home-manager")
    if is_registry_name(ref_part):
        entry = resolve_registry_name(ref_part)
        if entry:
            if entry['type'] == 'path':
                # Local path from registry - handle natively!
                return ResolvedInstallable(
                    is_local=True,
                    attr_part=attr_part,
                    flake_dir=Path(entry['path']).expanduser().resolve(),
                )
            else:
                # Remote ref from registry - passthrough to nix
                flake_ref = registry_entry_to_flake_ref(entry)
                return ResolvedInstallable(
                    is_local=False,
                    attr_part=attr_part,
                    flake_ref=flake_ref,
                )
        else:
            # Registry name not found - still try as remote ref
            # (nix might resolve it via its own registry)
            return ResolvedInstallable(
                is_local=False,
                attr_part=attr_part,
                flake_ref=ref_part,
            )

    # Fallback: treat as local path
    return ResolvedInstallable(
        is_local=True,
        attr_part=attr_part,
        flake_dir=Path(ref_part).resolve(),
    )


def _looks_like_system(s: str) -> bool:
    """Check if a string looks like a Nix system identifier (e.g., x86_64-linux)."""
    return '-' in s


def resolve_attr_path(attr_part: str, default_category: str, system: str) -> str:
    """Build full attribute path with system.

    Args:
        attr_part: e.g., "hello", "devShells.myshell", "hello.name"
        default_category: "packages" or "devShells"
        system: e.g., "x86_64-linux"

    Returns:
        Full attr path like "packages.x86_64-linux.hello"
    """
    # Known per-system output categories
    per_system_categories = ('packages', 'devShells', 'apps', 'checks', 'legacyPackages', 'formatter')
    # Known top-level (non-system) output categories
    top_level_categories = (
        'lib',
        'overlays',
        'nixosModules',
        'nixosConfigurations',
        'darwinModules',
        'darwinConfigurations',
        'homeManagerModules',
        'templates',
        'defaultTemplate',
        'self',
    )

    # Simple name like "hello" or "default" - most common case
    if '.' not in attr_part:
        return f'{default_category}.{system}.{attr_part}'

    parts = attr_part.split('.')
    first = parts[0]

    # Top-level outputs don't need system prefix: "lib.foo" stays "lib.foo"
    if first in top_level_categories:
        return attr_part

    # Per-system category (packages, devShells, etc.)
    if first in per_system_categories:
        # Check if system is already present (e.g., "packages.x86_64-linux.foo")
        if len(parts) >= 3 and _looks_like_system(parts[1]):
            return attr_part
        # Insert system: "packages.foo" -> "packages.{system}.foo"
        return f'{first}.{system}.{".".join(parts[1:])}'

    # Unknown first component with dots - pass through as-is
    # This allows custom top-level outputs like finixConfigurations, nixosConfigurations, etc.
    # The eval.nix has proper fallback logic if the path doesn't exist
    return attr_part


def _get_flake_info(flake_dir: Path) -> tuple[str, list[str]]:
    """Get system and input names in a single nix-instantiate call.

    Returns (system, input_names) tuple.
    """
    expr = f"""
    {{
      system = builtins.currentSystem;
      inputNames = builtins.attrNames ((import {flake_dir}/flake.nix).inputs or {{}});
    }}
    """
    result = subprocess.run(
        ['nix-instantiate', '--eval', '--expr', expr, '--json', '--strict'],
        capture_output=True,
        text=True,
        env=_get_clean_env(),
    )
    if result.returncode != 0:
        # Fallback for system
        import platform

        machine = platform.machine()
        system = platform.system().lower()
        return f'{machine}-{system}', []

    data = json.loads(result.stdout)
    return data['system'], data['inputNames']


def ensure_lock(flake_dir: Path, verbose: bool = False) -> None:
    """Ensure flake.lock exists with locked versions of flake inputs.

    Uses nix flake prefetch to lock inputs with proper revisions and hashes.
    Exits with error if unsupported flake features are detected.
    """
    import sys

    from .lock import ensure_lock as lock_inputs

    # Get input names from flake.nix
    _system, input_names = _get_flake_info(flake_dir)

    if not input_names:
        # No inputs at all - skip entirely
        return

    # Get full input specs for locking
    inputs = get_flake_inputs(flake_dir)

    if not inputs:
        # No inputs to lock - skip entirely
        return

    flake_lock = flake_dir / 'flake.lock'
    if not flake_lock.exists() and verbose:
        print('No flake.lock found. Locking flake inputs...', file=sys.stderr)

    try:
        if not lock_inputs(flake_dir, verbose=verbose):
            print('Warning: Failed to lock inputs', file=sys.stderr)
    except UnsupportedFlakeFeature as e:
        print(f'Error: {e}', file=sys.stderr)
        sys.exit(1)
