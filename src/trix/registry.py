"""Flake registry resolution.

Reads nix flake registries to resolve short names like 'nixpkgs' to their
full flake references. Supports:
- User registry: ~/.config/nix/registry.json
- System registry: /etc/nix/registry.json
- Global registry: https://channels.nixos.org/flake-registry.json (cached)

Registry entries can map to:
- Local paths (type: path) - handled natively by trix
- Remote refs (type: github, git, etc.) - passed through to nix
"""

import json
import os
import time
from pathlib import Path
from typing import TypedDict


class RegistryEntry(TypedDict, total=False):
    """A resolved registry entry."""

    type: str  # 'path', 'github', 'git', etc.
    path: str  # for type: path
    owner: str  # for type: github
    repo: str  # for type: github
    ref: str  # optional branch/tag
    rev: str  # optional commit
    url: str  # for type: git


# Cache for global registry (expires after 1 hour)
_global_registry_cache: dict | None = None
_global_registry_cache_time: float = 0
GLOBAL_REGISTRY_URL = 'https://channels.nixos.org/flake-registry.json'
CACHE_TTL = 3600  # 1 hour


def _get_user_registry_path() -> Path:
    """Get the user registry path."""
    config_home = os.environ.get('XDG_CONFIG_HOME', os.path.expanduser('~/.config'))
    return Path(config_home) / 'nix' / 'registry.json'


def _get_system_registry_path() -> Path:
    """Get the system registry path."""
    return Path('/etc/nix/registry.json')


def _load_registry_file(path: Path) -> dict:
    """Load a registry file, returning empty dict if not found."""
    if not path.exists():
        return {}
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return {}


def _fetch_global_registry() -> dict:
    """Fetch and cache the global registry."""
    global _global_registry_cache, _global_registry_cache_time

    now = time.time()
    if _global_registry_cache is not None and (now - _global_registry_cache_time) < CACHE_TTL:
        return _global_registry_cache

    try:
        import urllib.request

        with urllib.request.urlopen(GLOBAL_REGISTRY_URL, timeout=5) as response:
            data = json.loads(response.read().decode())
            _global_registry_cache = data
            _global_registry_cache_time = now
            return data
    except Exception:
        # Return cached version if available, otherwise empty
        return _global_registry_cache or {}


def _parse_registry_entry(entry: dict) -> RegistryEntry | None:
    """Parse a registry 'to' entry into a RegistryEntry."""
    to = entry.get('to', {})
    entry_type = to.get('type')

    if entry_type == 'path':
        return {'type': 'path', 'path': to.get('path', '')}

    elif entry_type == 'github':
        result: RegistryEntry = {
            'type': 'github',
            'owner': to.get('owner', ''),
            'repo': to.get('repo', ''),
        }
        if 'ref' in to:
            result['ref'] = to['ref']
        if 'rev' in to:
            result['rev'] = to['rev']
        return result

    elif entry_type == 'git':
        result: RegistryEntry = {
            'type': 'git',
            'url': to.get('url', ''),
        }
        if 'ref' in to:
            result['ref'] = to['ref']
        if 'rev' in to:
            result['rev'] = to['rev']
        return result

    return None


def _search_registry(registry: dict, name: str) -> RegistryEntry | None:
    """Search a registry for a name, return the resolved entry."""
    flakes = registry.get('flakes', [])
    for entry in flakes:
        from_spec = entry.get('from', {})
        if from_spec.get('type') == 'indirect' and from_spec.get('id') == name:
            return _parse_registry_entry(entry)
    return None


def resolve_registry_name(name: str, use_global: bool = True) -> RegistryEntry | None:
    """Resolve a registry name to its target.

    Searches in order:
    1. User registry (~/.config/nix/registry.json)
    2. System registry (/etc/nix/registry.json)
    3. Global registry (https://channels.nixos.org/flake-registry.json)

    Args:
        name: The registry name to resolve (e.g., 'nixpkgs')
        use_global: Whether to fetch the global registry (default True)

    Returns:
        RegistryEntry if found, None otherwise
    """
    # Check user registry first
    user_registry = _load_registry_file(_get_user_registry_path())
    result = _search_registry(user_registry, name)
    if result:
        return result

    # Check system registry
    system_registry = _load_registry_file(_get_system_registry_path())
    result = _search_registry(system_registry, name)
    if result:
        return result

    # Check global registry
    if use_global:
        global_registry = _fetch_global_registry()
        result = _search_registry(global_registry, name)
        if result:
            return result

    return None


def registry_entry_to_flake_ref(entry: RegistryEntry) -> str:
    """Convert a registry entry to a flake reference string.

    Args:
        entry: The registry entry

    Returns:
        A flake reference string like 'github:NixOS/nixpkgs' or '/path/to/flake'
    """
    if entry['type'] == 'path':
        return entry.get('path', '')

    elif entry['type'] == 'github':
        ref = f'github:{entry["owner"]}/{entry["repo"]}'
        if 'rev' in entry:
            ref += f'/{entry["rev"]}'
        elif 'ref' in entry:
            ref += f'/{entry["ref"]}'
        return ref

    elif entry['type'] == 'git':
        ref = f'git+{entry["url"]}'
        params = []
        if 'ref' in entry:
            params.append(f'ref={entry["ref"]}')
        if 'rev' in entry:
            params.append(f'rev={entry["rev"]}')
        if params:
            ref += '?' + '&'.join(params)
        return ref

    return ''


def is_registry_name(ref: str) -> bool:
    """Check if a reference looks like a registry name (not a path or full ref).

    Registry names are simple identifiers like 'nixpkgs', 'home-manager'.
    Not paths (., ./, /, ~) or full refs (github:, git+, path:).

    Args:
        ref: The reference to check

    Returns:
        True if it looks like a registry name
    """
    # Empty or path-like
    if not ref or ref.startswith(('.', '/', '~')):
        return False

    # Full flake reference (has colon like github: or git+)
    if ':' in ref:
        return False

    # Contains # (has attribute part)
    base = ref.split('#')[0]

    # Check if base is a simple identifier (alphanumeric + hyphen + underscore)
    return all(c.isalnum() or c in '-_' for c in base) and len(base) > 0


# --- Registry write operations ---


def _parse_flake_ref_to_entry(ref: str) -> dict:
    """Parse a flake reference string into a registry 'to' entry.

    Args:
        ref: A flake reference like '/path/to/flake', 'github:owner/repo', etc.

    Returns:
        A dict suitable for the 'to' field of a registry entry
    """
    # Local path
    if ref.startswith('/') or ref.startswith('~') or ref.startswith('./') or ref.startswith('../'):
        path = os.path.expanduser(ref)
        path = os.path.abspath(path)
        return {'type': 'path', 'path': path}

    # path: prefix
    if ref.startswith('path:'):
        path = os.path.expanduser(ref[5:])
        path = os.path.abspath(path)
        return {'type': 'path', 'path': path}

    # github: reference
    if ref.startswith('github:'):
        rest = ref[7:]
        # Handle query params
        query_params = {}
        if '?' in rest:
            rest, query = rest.split('?', 1)
            for part in query.split('&'):
                if '=' in part:
                    k, v = part.split('=', 1)
                    query_params[k] = v

        parts = rest.split('/')
        result = {'type': 'github', 'owner': parts[0], 'repo': parts[1]}
        if len(parts) > 2:
            result['ref'] = parts[2]
        if 'ref' in query_params:
            result['ref'] = query_params['ref']
        if 'rev' in query_params:
            result['rev'] = query_params['rev']
        return result

    # git+ reference
    if ref.startswith('git+'):
        url = ref[4:]
        query_params = {}
        if '?' in url:
            url, query = url.split('?', 1)
            for part in query.split('&'):
                if '=' in part:
                    k, v = part.split('=', 1)
                    query_params[k] = v

        result = {'type': 'git', 'url': url}
        if 'ref' in query_params:
            result['ref'] = query_params['ref']
        if 'rev' in query_params:
            result['rev'] = query_params['rev']
        return result

    # Fallback: treat as path
    path = os.path.expanduser(ref)
    path = os.path.abspath(path)
    return {'type': 'path', 'path': path}


def _save_user_registry(registry: dict) -> None:
    """Save the user registry file."""
    path = _get_user_registry_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, 'w') as f:
        json.dump(registry, f, indent=2)
        f.write('\n')


def list_all_registries(use_global: bool = True) -> list[tuple[str, str, RegistryEntry]]:
    """List all registry entries from all sources.

    Returns:
        List of (name, source, entry) tuples where source is 'user', 'system', or 'global'
    """
    results = []

    # User registry
    user_registry = _load_registry_file(_get_user_registry_path())
    for entry in user_registry.get('flakes', []) or []:
        from_spec = entry.get('from', {})
        if from_spec.get('type') == 'indirect':
            name = from_spec.get('id', '')
            parsed = _parse_registry_entry(entry)
            if parsed:
                results.append((name, 'user', parsed))

    # System registry
    system_registry = _load_registry_file(_get_system_registry_path())
    for entry in system_registry.get('flakes', []) or []:
        from_spec = entry.get('from', {})
        if from_spec.get('type') == 'indirect':
            name = from_spec.get('id', '')
            parsed = _parse_registry_entry(entry)
            if parsed:
                results.append((name, 'system', parsed))

    # Global registry
    if use_global:
        global_registry = _fetch_global_registry()
        for entry in global_registry.get('flakes', []) or []:
            from_spec = entry.get('from', {})
            if from_spec.get('type') == 'indirect':
                name = from_spec.get('id', '')
                parsed = _parse_registry_entry(entry)
                if parsed:
                    results.append((name, 'global', parsed))

    return results


def add_registry_entry(name: str, target: str) -> None:
    """Add an entry to the user registry.

    Args:
        name: The registry name (e.g., 'nixpkgs', 'myflake')
        target: The flake reference (e.g., '/path/to/flake', 'github:owner/repo')
    """
    user_registry = _load_registry_file(_get_user_registry_path())

    # Ensure structure
    if 'version' not in user_registry:
        user_registry['version'] = 2
    if 'flakes' not in user_registry or user_registry['flakes'] is None:
        user_registry['flakes'] = []

    # Remove existing entry with same name
    user_registry['flakes'] = [
        e
        for e in user_registry['flakes']
        if not (e.get('from', {}).get('type') == 'indirect' and e.get('from', {}).get('id') == name)
    ]

    # Add new entry
    new_entry = {'from': {'id': name, 'type': 'indirect'}, 'to': _parse_flake_ref_to_entry(target)}
    user_registry['flakes'].append(new_entry)

    _save_user_registry(user_registry)


def remove_registry_entry(name: str) -> bool:
    """Remove an entry from the user registry.

    Args:
        name: The registry name to remove

    Returns:
        True if entry was found and removed, False otherwise
    """
    user_registry = _load_registry_file(_get_user_registry_path())

    flakes = user_registry.get('flakes', []) or []
    original_count = len(flakes)

    # Filter out the entry
    user_registry['flakes'] = [
        e for e in flakes if not (e.get('from', {}).get('type') == 'indirect' and e.get('from', {}).get('id') == name)
    ]

    if len(user_registry['flakes']) < original_count:
        _save_user_registry(user_registry)
        return True

    return False
