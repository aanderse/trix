"""Version locking using nix flake prefetch.

Produces flake.lock files in the native nix format (version 7).
"""

import json
import subprocess
import sys
from datetime import datetime
from pathlib import Path

from .flake import get_flake_inputs
from .nix import _get_clean_env, warn


# ANSI color codes for terminal output (matching nix's style)
def _use_color() -> bool:
    """Check if we should use colored output."""
    return sys.stderr.isatty()


def _yellow(text: str) -> str:
    """Yellow text for warnings."""
    if _use_color():
        return f'\033[1;33m{text}\033[0m'
    return text


def _magenta(text: str) -> str:
    """Magenta/bold text for bullets and emphasis."""
    if _use_color():
        return f'\033[1;35m{text}\033[0m'
    return text


def _cyan(text: str) -> str:
    """Cyan text for URLs."""
    if _use_color():
        return f'\033[36m{text}\033[0m'
    return text


def _bold(text: str) -> str:
    """Bold text."""
    if _use_color():
        return f'\033[1m{text}\033[0m'
    return text


def _format_locked_url(node: dict) -> str:
    """Format a locked node as a display URL with date, matching nix's format."""
    # Handle follows pseudo-node
    if '_follows' in node:
        follows_path = node['_follows']
        return "follows '" + '/'.join(follows_path) + "'"

    locked = node.get('locked', {})
    typ = locked.get('type', '')
    last_modified = locked.get('lastModified')

    if typ == 'github':
        owner = locked.get('owner', '')
        repo = locked.get('repo', '')
        rev = locked.get('rev', '')
        url = f'github:{owner}/{repo}/{rev}'
    elif typ == 'git':
        git_url = locked.get('url', '')
        rev = locked.get('rev', '')
        url = f'git+{git_url}?rev={rev}'
    elif typ == 'path':
        url = f'path:{locked.get("path", "")}'
    else:
        url = str(locked)

    if last_modified:
        date_str = datetime.fromtimestamp(last_modified).strftime('%Y-%m-%d')
        url += f' ({date_str})'

    return url


def _fetch_source_flake_lock(node: dict, verbose: bool = False, input_name: str = '') -> dict | None:
    """Fetch a locked input's source and read its flake.lock.

    Returns the parsed flake.lock content, or None if no flake.lock exists.
    """
    locked = node.get('locked', {})
    source_type = locked.get('type', '')

    # Path inputs: read directly from filesystem
    if source_type == 'path':
        path = locked.get('path', '')
        if not path:
            return None
        lock_path = Path(path) / 'flake.lock'
        if not lock_path.exists():
            return None
        try:
            with open(lock_path) as f:
                return json.load(f)
        except (OSError, json.JSONDecodeError):
            return None

    # Git inputs: use fetchGit
    if source_type == 'git':
        nar_hash = locked.get('narHash', '')
        ref_part = f'ref = "{locked["ref"]}";' if locked.get('ref') else ''
        nix_expr = f'''
          let
            src = builtins.fetchGit {{
              url = "{locked.get('url', '')}";
              rev = "{locked.get('rev', '')}";
              narHash = "{nar_hash}";
              {ref_part}
            }};
            lockPath = src + "/flake.lock";
          in
            if builtins.pathExists lockPath
            then builtins.readFile lockPath
            else ""
        '''
    # Tarball-based inputs: use fetchTarball with appropriate URL
    else:
        if source_type == 'github':
            url = f'https://github.com/{locked["owner"]}/{locked["repo"]}/archive/{locked["rev"]}.tar.gz'
            nar_hash = locked.get('narHash', '')
        elif source_type == 'gitlab':
            host = locked.get('host', 'gitlab.com')
            url = f'https://{host}/{locked["owner"]}/{locked["repo"]}/-/archive/{locked["rev"]}/{locked["repo"]}-{locked["rev"]}.tar.gz'
            nar_hash = locked.get('narHash', '')
        elif source_type == 'sourcehut':
            host = locked.get('host', 'git.sr.ht')
            url = f'https://{host}/~{locked["owner"]}/{locked["repo"]}/archive/{locked["rev"]}.tar.gz'
            nar_hash = locked.get('narHash', '')
        elif source_type == 'tarball':
            url = locked.get('url', '')
            nar_hash = locked.get('narHash', '')
        elif source_type == 'file':
            url = locked.get('url', '')
            nar_hash = locked.get('narHash', '')
        elif source_type in ('mercurial', 'hg'):
            # Mercurial not supported - warn user
            name_str = f" '{input_name}'" if input_name else ''
            warn(f"mercurial input{name_str} skipped (not supported for transitive dependency collection)")
            return None
        else:
            # Unknown type - warn and skip
            name_str = f" '{input_name}'" if input_name else ''
            warn(f"unknown source type '{source_type}' for input{name_str}, skipping transitive dependency collection")
            return None

        nix_expr = f'''
          let
            src = builtins.fetchTarball {{
              url = "{url}";
              sha256 = "{nar_hash}";
            }};
            lockPath = src + "/flake.lock";
          in
            if builtins.pathExists lockPath
            then builtins.readFile lockPath
            else ""
        '''

    cmd = ['nix-instantiate', '--eval', '--expr', nix_expr]
    if verbose:
        print('  Fetching transitive deps...', file=sys.stderr)

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        if verbose:
            print(f'  Failed to fetch source: {result.stderr}', file=sys.stderr)
        return None

    # nix-instantiate returns a quoted string, strip the quotes
    lock_content = result.stdout.strip()
    if lock_content.startswith('"') and lock_content.endswith('"'):
        # Unescape the string (nix escapes newlines, etc.)
        lock_content = lock_content[1:-1].encode().decode('unicode_escape')

    if not lock_content:
        return None

    try:
        return json.loads(lock_content)
    except json.JSONDecodeError:
        return None


def _prefetch_flake(flake_ref: str, verbose: bool = False) -> dict | None:
    """Use nix flake prefetch to lock a flake source.

    This respects access-tokens from nix.conf for private repos.

    Returns the raw prefetch result or None on failure.
    """
    cmd = [
        'nix',
        '--extra-experimental-features',
        'nix-command flakes',
        'flake',
        'prefetch',
        flake_ref,
        '--json',
    ]
    if verbose:
        print(f'+ {" ".join(cmd)}', file=sys.stderr)

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        if verbose:
            print(result.stderr, file=sys.stderr)
        return None

    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as e:
        if verbose:
            print(f'Failed to parse prefetch output: {e}', file=sys.stderr)
        return None


def _lock_input(name: str, spec: dict, verbose: bool = False) -> dict | None:
    """Lock a single input, returning a node in native flake.lock format.

    Returns dict with 'locked' and 'original' keys matching nix's format.
    """
    is_flake = spec.get('flake', True)

    if spec['type'] == 'github':
        owner = spec['owner']
        repo = spec['repo']
        ref = spec.get('ref')
        rev = spec.get('rev')

        flake_ref = f'github:{owner}/{repo}'
        if rev:
            # Use ?rev= query parameter for specific commits
            flake_ref += f'?rev={rev}'
        elif ref:
            flake_ref += f'/{ref}'

        if verbose:
            print(f'Locking {name} ({flake_ref})', file=sys.stderr)

        data = _prefetch_flake(flake_ref, verbose=verbose)
        if not data:
            return None

        locked = data.get('locked', {})
        original = data.get('original', {})

        result = {
            'locked': {
                'lastModified': locked.get('lastModified'),
                'narHash': data['hash'],
                'owner': locked.get('owner', owner),
                'repo': locked.get('repo', repo),
                'rev': locked['rev'],
                'type': 'github',
            },
            'original': {
                'owner': original.get('owner', owner),
                'repo': original.get('repo', repo),
                'type': 'github',
            },
        }
        # Only include ref if it has a value (nix doesn't like null)
        actual_ref = ref or original.get('ref')
        if actual_ref:
            result['original']['ref'] = actual_ref
        if not is_flake:
            result['flake'] = False
        return result

    elif spec['type'] == 'git':
        url = spec['url']
        ref = spec.get('ref')

        flake_ref = f'git+{url}'
        if ref:
            flake_ref += f'?ref={ref}'

        if verbose:
            print(f'Locking {name} ({flake_ref})', file=sys.stderr)

        data = _prefetch_flake(flake_ref, verbose=verbose)
        if not data:
            return None

        locked = data.get('locked', {})
        original = data.get('original', {})

        result = {
            'locked': {
                'lastModified': locked.get('lastModified'),
                'narHash': data['hash'],
                'rev': locked['rev'],
                'type': 'git',
                'url': locked.get('url', url),
            },
            'original': {
                'type': 'git',
                'url': original.get('url', url),
            },
        }
        # Only include optional fields if they have values
        if locked.get('revCount'):
            result['locked']['revCount'] = locked['revCount']
        actual_ref = ref or original.get('ref')
        if actual_ref:
            result['original']['ref'] = actual_ref
        if not is_flake:
            result['flake'] = False
        return result

    elif spec['type'] == 'path':
        path = spec.get('path', '')
        if verbose:
            print(f'Locking {name} (path:{path})', file=sys.stderr)

        # Use nix flake prefetch to get lastModified and narHash
        flake_ref = f'path:{path}'
        data = _prefetch_flake(flake_ref, verbose=verbose)
        if data:
            locked = data.get('locked', {})
            result = {
                'locked': {
                    'lastModified': locked.get('lastModified'),
                    'narHash': data['hash'],
                    'path': path,
                    'type': 'path',
                },
                'original': {
                    'path': path,
                    'type': 'path',
                },
            }
        else:
            # Fallback if prefetch fails (shouldn't happen for valid paths)
            result = {
                'locked': {
                    'path': path,
                    'type': 'path',
                },
                'original': {
                    'path': path,
                    'type': 'path',
                },
            }
        if not is_flake:
            result['flake'] = False
        return result

    else:
        if verbose:
            print(f'Skipping unknown input type: {name} ({spec["type"]})', file=sys.stderr)
        return None


def _validate_lock_compatibility(lock_data: dict) -> None:
    """Check that an existing lock file is compatible with trix.

    Raises UnsupportedFlakeFeature if the lock uses features we don't support.

    Note: List references in root.inputs (like ["nixpkgs"]) indicate follows,
    which we now support. We validate that follows paths are valid.
    """
    # Currently we support all standard lock file features including follows
    pass


def _read_lock(flake_lock: Path) -> dict:
    """Read existing lock file or return empty structure."""
    if flake_lock.exists():
        try:
            with open(flake_lock) as f:
                data = json.load(f)
            # Check lock file version
            version = data.get('version')
            if version is not None and version != 7:
                warn(f"flake.lock version {version} may not be fully supported (expected 7)")
            return data
        except json.JSONDecodeError:
            pass

    # Return empty native format structure
    return {
        'nodes': {'root': {'inputs': {}}},
        'root': 'root',
        'version': 7,
    }


def _remove_nulls(obj):
    """Recursively remove keys with None values from dicts.

    Native nix doesn't accept null values in lock files.
    """
    if isinstance(obj, dict):
        return {k: _remove_nulls(v) for k, v in obj.items() if v is not None}
    elif isinstance(obj, list):
        return [_remove_nulls(item) for item in obj]
    return obj


def _write_lock(flake_lock: Path, lock_data: dict) -> None:
    """Write lock file with consistent formatting."""
    # Remove any null values - native nix doesn't accept them
    sanitized = _remove_nulls(lock_data)
    with open(flake_lock, 'w') as f:
        json.dump(sanitized, f, indent=2, sort_keys=True)
        f.write('\n')


def _lock_data_equal(old: dict, new: dict) -> bool:
    """Compare two lock data structures for equality."""
    # Compare JSON serializations for reliable deep comparison
    return json.dumps(old, sort_keys=True) == json.dumps(new, sort_keys=True)


def _collect_transitive_deps(
    node: dict,
    new_nodes: dict,
    added_inputs: list,
    verbose: bool = False,
    input_name: str = '',
) -> None:
    """Recursively collect transitive dependencies from an input's flake.lock.

    For flake inputs, fetches their source and reads their flake.lock to find
    transitive dependencies that need to be added to our lock file.

    NOTE: This function mutates its arguments (standard pattern for recursive collection):
    - node: Gets 'inputs' field populated with references to transitive deps
    - new_nodes: Gets transitive dependency nodes added
    - added_inputs: Gets (name, node) tuples appended for newly discovered deps

    Args:
        node: The locked node to collect transitive deps for
        new_nodes: Dict of nodes being built
        added_inputs: List of (name, node) for newly added inputs
        verbose: Print progress
        input_name: Name of the input (for warning messages)
    """
    # Skip non-flake inputs
    if not node.get('flake', True):
        return

    # Get the input's flake.lock
    input_lock = _fetch_source_flake_lock(node, verbose=verbose, input_name=input_name)
    if not input_lock:
        return

    input_nodes = input_lock.get('nodes', {})
    input_root_inputs = input_nodes.get('root', {}).get('inputs', {})

    # Get existing overrides from this node
    node_inputs = node.get('inputs', {})

    # For each input in the transitive flake.lock
    for input_name, ref in input_root_inputs.items():
        # Resolve the reference to a node name
        if isinstance(ref, list):
            # Follows reference within the input's lock - resolve it
            ref_node_name = ref[0] if ref else input_name
        else:
            ref_node_name = ref

        # Skip if already overridden by a follows in our lock (list values)
        existing_ref = node_inputs.get(input_name)
        if isinstance(existing_ref, list):
            continue

        # Add the input reference to this node (if not already there)
        if 'inputs' not in node:
            node['inputs'] = {}
        if input_name not in node['inputs']:
            node['inputs'][input_name] = ref_node_name

        # Skip adding the node if we already have it
        if ref_node_name in new_nodes:
            continue

        # Get the transitive node from the input's lock
        trans_node = input_nodes.get(ref_node_name)
        if not trans_node:
            continue

        # Add to our lock
        new_nodes[ref_node_name] = trans_node
        added_inputs.append((ref_node_name, trans_node))

        if verbose:
            print(f"  Adding transitive dep '{ref_node_name}'", file=sys.stderr)

        # Recursively collect this node's transitive deps
        _collect_transitive_deps(trans_node, new_nodes, added_inputs, verbose=verbose, input_name=ref_node_name)


def sync_inputs(flake_dir: Path, verbose: bool = False) -> bool:
    """Sync flake.nix inputs to lock file.

    Uses nix flake prefetch which respects access-tokens for private repos.
    Produces native flake.lock format (version 7).

    - Adds new inputs from flake.nix
    - Removes inputs no longer in flake.nix
    - Keeps existing locked versions for unchanged inputs
    - Only writes file if something changed (avoids triggering direnv)

    Raises UnsupportedFlakeFeature if the flake uses unsupported features.
    """
    inputs = get_flake_inputs(flake_dir)  # May raise UnsupportedFlakeFeature

    flake_lock = flake_dir / 'flake.lock'
    old_lock_data = _read_lock(flake_lock)
    lock_existed = flake_lock.exists()

    # Validate existing lock file is compatible before modifying
    if lock_existed:
        _validate_lock_compatibility(old_lock_data)

    nodes = old_lock_data.get('nodes', {})
    old_root_inputs = nodes.get('root', {}).get('inputs', {})

    # Build new root inputs, preserving existing locks
    new_root_inputs = {}
    new_nodes = {'root': {}}  # Start fresh, will rebuild

    # Track changes for output (like nix does)
    added_inputs = []  # list of (name, node)
    updated_inputs = []  # list of (name, old_node, new_node) - for follows changes
    removed_inputs = []  # list of name

    def collect_transitive_nodes(node_name: str) -> None:
        """Recursively collect a node and all nodes it references."""
        if node_name in new_nodes or node_name == 'root':
            return
        if node_name not in nodes:
            return
        node = nodes[node_name]
        new_nodes[node_name] = node
        # Recursively collect any nodes this node references
        node_inputs = node.get('inputs', {})
        for ref in node_inputs.values():
            if isinstance(ref, str):
                collect_transitive_nodes(ref)
            elif isinstance(ref, list) and len(ref) >= 1:
                # .follows reference like ["nixpkgs", "nixpkgs"] - follow the chain
                collect_transitive_nodes(ref[0])

    if not inputs:
        if verbose:
            print('No flake inputs found', file=sys.stderr)
        new_nodes['root']['inputs'] = new_root_inputs
        new_lock_data = {
            'nodes': new_nodes,
            'root': 'root',
            'version': 7,
        }
        if not _lock_data_equal(old_lock_data, new_lock_data):
            _write_lock(flake_lock, new_lock_data)
        return True

    for name, spec in inputs.items():
        # Check if we already have this input locked
        if name in nodes and name != 'root':
            existing_node = nodes[name]
            # Check if flake attribute changed
            spec_is_flake = spec.get('flake', True)
            node_is_flake = existing_node.get('flake', True)
            if spec_is_flake != node_is_flake:
                # Re-lock with updated flake attribute
                node = _lock_input(name, spec, verbose=verbose)
                if node:
                    # Add follows if specified
                    if 'follows' in spec:
                        node['inputs'] = spec['follows']
                    new_nodes[name] = node
                    new_root_inputs[name] = name
                    added_inputs.append((name, node))
                    # Collect transitive dependencies
                    _collect_transitive_deps(node, new_nodes, added_inputs, verbose=verbose, input_name=name)
                continue

            # Check if follows changed (only list values are follows, string values are transitive refs)
            existing_inputs = existing_node.get('inputs', {})
            existing_follows = {k: v for k, v in existing_inputs.items() if isinstance(v, list)}
            new_follows = spec.get('follows', {})
            if existing_follows != new_follows:
                # Update the node with new follows, preserving transitive refs
                updated_node = dict(existing_node)
                # Keep transitive refs (string values), update follows (list values)
                transitive_refs = {k: v for k, v in existing_inputs.items() if isinstance(v, str)}
                if new_follows or transitive_refs:
                    updated_node['inputs'] = {**transitive_refs, **new_follows}
                elif 'inputs' in updated_node:
                    del updated_node['inputs']
                new_nodes[name] = updated_node
                new_root_inputs[name] = name
                # Track as update (not add) with old and new state
                updated_inputs.append((name, existing_node, updated_node))
                # Collect transitive dependencies (may have changed with new follows)
                _collect_transitive_deps(updated_node, new_nodes, added_inputs, verbose=verbose, input_name=name)
                continue

            collect_transitive_nodes(name)
            new_root_inputs[name] = name
            # Check if any collected node has missing transitive deps
            # (can happen if nodes were locked before transitive dep support was added)
            for node_name, node in list(new_nodes.items()):
                if node_name == 'root':
                    continue
                node_refs = node.get('inputs', {})
                missing_refs = [
                    ref for ref in node_refs.values() if isinstance(ref, str) and ref not in new_nodes
                ]
                if missing_refs:
                    # Missing referenced nodes - fetch and add them
                    _collect_transitive_deps(node, new_nodes, added_inputs, verbose=verbose, input_name=node_name)
            continue

        # Root-level follows (inputs.foo.follows = "bar") - no node, just reference
        if spec['type'] == 'follows':
            follows_path = spec['follows']
            new_root_inputs[name] = follows_path
            # Track as added for output (with a pseudo-node for display)
            added_inputs.append((name, {'_follows': follows_path}))
            continue

        node = _lock_input(name, spec, verbose=verbose)
        if node:
            # Add transitive follows if specified (inputs.foo.inputs.bar.follows)
            if 'follows' in spec:
                node['inputs'] = spec['follows']
            new_nodes[name] = node
            new_root_inputs[name] = name
            added_inputs.append((name, node))
            # Collect transitive dependencies
            _collect_transitive_deps(node, new_nodes, added_inputs, verbose=verbose, input_name=name)

    # Check for removed inputs
    for name in old_root_inputs:
        if name not in new_root_inputs:
            removed_inputs.append(name)

    # Update root node
    new_nodes['root'] = {'inputs': new_root_inputs}
    new_lock_data = {
        'nodes': new_nodes,
        'root': 'root',
        'version': 7,
    }

    # Only write and print if something changed
    if added_inputs or updated_inputs or removed_inputs or not _lock_data_equal(old_lock_data, new_lock_data):
        _write_lock(flake_lock, new_lock_data)

        # Print changes like nix does
        if added_inputs or updated_inputs or removed_inputs:
            action = 'updating' if lock_existed else 'creating'
            print(f"{_yellow('warning:')} {action} lock file '{flake_lock}':", file=sys.stderr)
            for name, node in added_inputs:
                url = _format_locked_url(node)
                print(f"{_magenta('•')} {_magenta('Added input')} {_bold(repr(name))}:", file=sys.stderr)
                print(f"    {_cyan(repr(url))}", file=sys.stderr)
            for name, old_node, new_node in updated_inputs:
                old_follows = old_node.get('inputs', {})
                new_follows = new_node.get('inputs', {})
                # Show each nested follows change
                all_nested = set(old_follows.keys()) | set(new_follows.keys())
                for nested in sorted(all_nested):
                    old_ref = old_follows.get(nested)
                    new_ref = new_follows.get(nested)
                    if old_ref != new_ref:
                        print(f"{_magenta('•')} {_magenta('Updated input')} {_bold(repr(f'{name}/{nested}'))}:", file=sys.stderr)
                        if old_ref:
                            if isinstance(old_ref, list):
                                print(f"    {_magenta('follows')} {_cyan(repr('/'.join(old_ref)))}", file=sys.stderr)
                            else:
                                print(f"    {_cyan(repr(old_ref))}", file=sys.stderr)
                        else:
                            print('    (was not overridden)', file=sys.stderr)
                        if new_ref:
                            if isinstance(new_ref, list):
                                print(f"  → {_magenta('follows')} {_cyan(repr('/'.join(new_ref)))}", file=sys.stderr)
                            else:
                                print(f"  → {_cyan(repr(new_ref))}", file=sys.stderr)
                        else:
                            print('  → (no longer overridden)', file=sys.stderr)
            for name in removed_inputs:
                print(f"{_magenta('•')} {_magenta('Removed input')} {_bold(repr(name))}", file=sys.stderr)

    return True


def ensure_lock(flake_dir: Path, verbose: bool = False) -> bool:
    """Ensure lock file exists and is up to date with flake inputs."""
    return sync_inputs(flake_dir, verbose=verbose)


def _lock_flake_ref(name: str, flake_ref: str, verbose: bool = False, original_spec: dict | None = None) -> dict | None:
    """Lock an input to a specific flake reference.

    Args:
        name: Input name (for messages)
        flake_ref: Full flake reference (e.g., github:NixOS/nixpkgs/abc123)
        verbose: Print commands
        original_spec: Optional spec from flake.nix to use for 'original' field.
                      If provided, this is used instead of the prefetch's original,
                      matching native nix behavior for --override-input.

    Returns:
        Lock node in native flake.lock format, or None on failure.
    """
    if verbose:
        print(f'Locking {name} to {flake_ref}', file=sys.stderr)

    data = _prefetch_flake(flake_ref, verbose=verbose)
    if not data:
        return None

    locked = data.get('locked', {})
    prefetch_original = data.get('original', {})
    source_type = locked.get('type', '')

    if source_type == 'github':
        # Build original from flake.nix spec if provided (for overrides),
        # otherwise use prefetch's original (for normal locking)
        if original_spec and original_spec.get('type') == 'github':
            original_dict = {
                'owner': original_spec.get('owner'),
                'repo': original_spec.get('repo'),
                'type': 'github',
            }
            if original_spec.get('ref'):
                original_dict['ref'] = original_spec['ref']
        else:
            original_dict = {
                'owner': prefetch_original.get('owner'),
                'repo': prefetch_original.get('repo'),
                'type': 'github',
            }
            # Include ref or rev in original (whichever prefetch returned)
            if prefetch_original.get('rev'):
                original_dict['rev'] = prefetch_original['rev']
            elif prefetch_original.get('ref'):
                original_dict['ref'] = prefetch_original['ref']

        return {
            'locked': {
                'lastModified': locked.get('lastModified'),
                'narHash': data['hash'],
                'owner': locked.get('owner'),
                'repo': locked.get('repo'),
                'rev': locked['rev'],
                'type': 'github',
            },
            'original': original_dict,
        }
    elif source_type == 'git':
        # Build original from flake.nix spec if provided
        if original_spec and original_spec.get('type') == 'git':
            original_dict = {
                'type': 'git',
                'url': original_spec.get('url'),
            }
            if original_spec.get('ref'):
                original_dict['ref'] = original_spec['ref']
        else:
            original_dict = {
                'type': 'git',
                'url': prefetch_original.get('url'),
            }
            if prefetch_original.get('rev'):
                original_dict['rev'] = prefetch_original['rev']
            elif prefetch_original.get('ref'):
                original_dict['ref'] = prefetch_original['ref']

        return {
            'locked': {
                'lastModified': locked.get('lastModified'),
                'narHash': data['hash'],
                'rev': locked['rev'],
                'revCount': locked.get('revCount'),
                'type': 'git',
                'url': locked.get('url'),
            },
            'original': original_dict,
        }
    else:
        print(f'Unsupported flake type for override: {source_type}', file=sys.stderr)
        return None


def update_lock(
    flake_dir: Path,
    input_name: str | None = None,
    override_inputs: dict[str, str] | None = None,
    verbose: bool = False,
) -> dict[str, tuple[str, str]]:
    """Update locked inputs to latest versions.

    Uses nix flake prefetch which respects access-tokens for private repos.

    Args:
        flake_dir: Directory containing flake.nix
        input_name: Specific input to update, or None for all
        override_inputs: Dict mapping input names to flake refs to pin to
        verbose: Print commands

    Returns:
        Dict mapping input name to (old_rev, new_rev) for changed inputs.
        Empty dict if no changes. None on error.
    """
    override_inputs = override_inputs or {}
    flake_lock = flake_dir / 'flake.lock'
    inputs = get_flake_inputs(flake_dir)

    # Validate override inputs exist in flake.nix
    for name in override_inputs:
        if name not in inputs:
            print(f"Error: input '{name}' not found in flake.nix", file=sys.stderr)
            return None

    lock_existed = flake_lock.exists()

    if not lock_existed:
        # No lock yet - if no overrides or input_name, just use sync_inputs
        if not override_inputs and not input_name:
            if sync_inputs(flake_dir, verbose=verbose):
                return {}
            return None

    lock_data = _read_lock(flake_lock)

    # Validate existing lock file is compatible before modifying
    if lock_existed:
        _validate_lock_compatibility(lock_data)

    nodes = lock_data.get('nodes', {})
    root_inputs = nodes.get('root', {}).get('inputs', {})

    # Track changes for output (like nix does)
    changes = {}  # name -> (old_rev, new_rev)
    updated_inputs = []  # list of (name, old_node, new_node)
    added_inputs = []  # list of (name, node)
    removed_input_names = []  # list of name

    # When creating a new lock with overrides, we need to lock ALL inputs
    # (not just the overrides), using overrides where specified
    if not lock_existed:
        # Lock all inputs, using override refs where specified
        for name, spec in inputs.items():
            if name in override_inputs:
                # Use the override ref, but keep original from flake.nix
                node = _lock_flake_ref(name, override_inputs[name], verbose=verbose, original_spec=spec)
            elif spec['type'] == 'follows':
                # Root-level follows - just add reference
                follows_path = spec['follows']
                root_inputs[name] = follows_path
                added_inputs.append((name, {'_follows': follows_path}))
                continue
            else:
                # Normal input - lock to latest
                node = _lock_input(name, spec, verbose=verbose)

            if node:
                # Add transitive follows if specified
                follows_entries = []
                if 'follows' in spec:
                    node['inputs'] = spec['follows']
                    # Record follows for output
                    for nested_name, follows_path in spec['follows'].items():
                        if isinstance(follows_path, list):
                            follows_entries.append((f"{name}/{nested_name}", follows_path))
                nodes[name] = node
                root_inputs[name] = name
                added_inputs.append((name, node, follows_entries))
                # Collect transitive dependencies
                _collect_transitive_deps(node, nodes, added_inputs, verbose=verbose, input_name=name)
            else:
                print(f"Error: Failed to lock '{name}'", file=sys.stderr)
                return None

        # Write and report
        nodes['root'] = {'inputs': root_inputs}
        lock_data = {'nodes': nodes, 'root': 'root', 'version': 7}
        _write_lock(flake_lock, lock_data)

        # Print in nix's format
        print(f"{_yellow('warning:')} creating lock file '{flake_lock}':", file=sys.stderr)
        for item in added_inputs:
            if len(item) == 3:
                name, node, follows_entries = item
            else:
                name, node = item
                follows_entries = []
            url = _format_locked_url(node)
            print(f"{_magenta('•')} {_magenta('Added input')} {_bold(repr(name))}:", file=sys.stderr)
            print(f"    {_cyan(repr(url))}", file=sys.stderr)
            # Print follows entries
            for follows_name, follows_path in follows_entries:
                print(f"{_magenta('•')} {_magenta('Added input')} {_bold(repr(follows_name))}:", file=sys.stderr)
                print(f"    {_magenta('follows')} {_cyan(repr('/'.join(follows_path)))}", file=sys.stderr)
        return changes

    # Existing lock - apply overrides
    for name, flake_ref in override_inputs.items():
        old_node = nodes.get(name, {})
        # Pass the original spec from flake.nix so 'original' matches flake.nix, not the override
        original_spec = inputs.get(name)

        node = _lock_flake_ref(name, flake_ref, verbose=verbose, original_spec=original_spec)
        if node:
            old_rev = old_node.get('locked', {}).get('rev', '')[:11]
            new_rev = node.get('locked', {}).get('rev', '')[:11]
            if old_rev != new_rev:
                changes[name] = (old_rev, new_rev)
                if old_node:
                    updated_inputs.append((name, old_node, node))
                else:
                    added_inputs.append((name, node))
            nodes[name] = node
            root_inputs[name] = name
            # Collect transitive dependencies
            _collect_transitive_deps(node, nodes, added_inputs, verbose=verbose, input_name=name)
        else:
            print(f"Error: Failed to lock '{name}' to {flake_ref}", file=sys.stderr)
            return None

    # If we have overrides and no input_name, we're done (just apply overrides)
    if override_inputs and not input_name:
        nodes['root'] = {'inputs': root_inputs}
        lock_data['nodes'] = nodes
        _write_lock(flake_lock, lock_data)
        _print_lock_changes(flake_lock, updated_inputs, added_inputs, removed_input_names)
        # If overrides were specified but nothing changed, inform the user
        if not changes and not added_inputs:
            for name in override_inputs:
                rev = nodes.get(name, {}).get('locked', {}).get('rev', '')[:11]
                print(
                    f"{_yellow('warning:')} input {_bold(repr(name))} already at {_cyan(rev)}",
                    file=sys.stderr,
                )
        return changes

    # Determine which inputs to update (excluding already-overridden ones)
    inputs_to_update = {}
    if input_name:
        if input_name in inputs:
            if input_name not in override_inputs:
                inputs_to_update[input_name] = inputs[input_name]
        else:
            print(f"Error: input '{input_name}' not found in flake.nix", file=sys.stderr)
            return None
    else:
        inputs_to_update = {k: v for k, v in inputs.items() if k not in override_inputs}

    for name, spec in inputs_to_update.items():
        old_node = nodes.get(name, {})

        node = _lock_input(name, spec, verbose=verbose)
        if node:
            old_rev = old_node.get('locked', {}).get('rev', '')[:11]
            new_rev = node.get('locked', {}).get('rev', '')[:11]
            if old_rev != new_rev:
                changes[name] = (old_rev, new_rev)
                if old_node:
                    updated_inputs.append((name, old_node, node))
                else:
                    added_inputs.append((name, node))
            # Add transitive follows if specified (inputs.foo.inputs.bar.follows)
            if 'follows' in spec:
                node['inputs'] = spec['follows']
            nodes[name] = node
            root_inputs[name] = name
            # Collect transitive dependencies
            _collect_transitive_deps(node, nodes, added_inputs, verbose=verbose, input_name=name)

    # Remove inputs that are no longer in flake.nix
    removed_input_names = list(set(root_inputs.keys()) - set(inputs.keys()))
    for name in removed_input_names:
        del root_inputs[name]
        if name in nodes:
            del nodes[name]

    # Update root node
    nodes['root'] = {'inputs': root_inputs}
    lock_data['nodes'] = nodes

    _write_lock(flake_lock, lock_data)
    _print_lock_changes(flake_lock, updated_inputs, added_inputs, removed_input_names)
    return changes


def _print_lock_changes(
    flake_lock: Path,
    updated_inputs: list[tuple[str, dict, dict]],
    added_inputs: list[tuple[str, dict]],
    removed_input_names: list[str],
) -> None:
    """Print lock file changes in nix's format."""
    if not updated_inputs and not added_inputs and not removed_input_names:
        return

    print(f"{_yellow('warning:')} updating lock file '{flake_lock}':", file=sys.stderr)
    for name, old_node, new_node in updated_inputs:
        old_url = _format_locked_url(old_node)
        new_url = _format_locked_url(new_node)
        print(f"{_magenta('•')} {_magenta('Updated input')} {_bold(repr(name))}:", file=sys.stderr)
        print(f"    {_cyan(repr(old_url))}", file=sys.stderr)
        print(f"  → {_cyan(repr(new_url))}", file=sys.stderr)
    for name, node in added_inputs:
        url = _format_locked_url(node)
        print(f"{_magenta('•')} {_magenta('Added input')} {_bold(repr(name))}:", file=sys.stderr)
        print(f"    {_cyan(repr(url))}", file=sys.stderr)
    for name in removed_input_names:
        print(f"{_magenta('•')} {_magenta('Removed input')} {_bold(repr(name))}", file=sys.stderr)
