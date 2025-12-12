"""Nix command wrappers."""

import json
import os
import subprocess
import sys
from functools import lru_cache
from pathlib import Path


# Empty lock expression for flakes without a lock file
EMPTY_LOCK_EXPR = '{ nodes = { root = { inputs = {}; }; }; root = "root"; version = 7; }'


def _get_lock_expr(flake_dir: Path) -> str:
    """Get the Nix expression to load the flake lock file.

    Returns either an expression to read the existing lock file,
    or an empty lock structure if no lock file exists.
    """
    lock_file = flake_dir / 'flake.lock'
    if lock_file.exists():
        return f'builtins.fromJSON (builtins.readFile {flake_dir}/flake.lock)'
    return EMPTY_LOCK_EXPR


def _attr_to_nix_list(attr: str) -> str:
    """Convert a dotted attribute path to a Nix list expression.

    Examples:
        "packages.x86_64-linux.hello" -> '["packages" "x86_64-linux" "hello"]'
        "" -> "[]"
    """
    parts = [p for p in attr.split('.') if p]
    if not parts:
        return '[]'
    return '[' + ' '.join(f'"{p}"' for p in parts) + ']'


def _flake_eval_preamble(flake_dir: Path) -> str:
    """Generate the common Nix let-bindings for flake evaluation.

    Returns Nix code that sets up: system, flake, lock, inputs, outputs.
    Also includes the hasPath helper function.
    """
    nix_dir = get_nix_dir()
    system = get_system()
    lock_expr = _get_lock_expr(flake_dir)

    return f'''
      system = "{system}";
      flake = import {flake_dir}/flake.nix;
      lock = {lock_expr};
      inputs = import {nix_dir}/inputs.nix {{
        inherit lock system;
        flakeDirPath = {flake_dir};
      }};
      outputs = flake.outputs (inputs // {{ self = inputs.self // outputs; }});

      # Check if a nested path exists in an attrset
      hasPath = path: obj:
        if path == [] then true
        else if builtins.isAttrs obj && obj ? ${{builtins.head path}}
        then hasPath (builtins.tail path) obj.${{builtins.head path}}
        else false;

      # Get a value at a nested path
      getPath = path: obj:
        builtins.foldl' (o: k: o.${{k}}) obj path;
    '''


def _get_clean_env() -> dict:
    """Get environment suitable for spawning nix commands.

    Removes TMPDIR to let nix/bash use the system default (/tmp).
    This avoids issues where TMPDIR points to a directory created by
    a parent nix-shell that may be cleaned up unexpectedly.
    """
    env = os.environ.copy()
    env.pop('TMPDIR', None)
    return env


def warn(msg: str) -> None:
    """Print a warning message to stderr in nix style."""
    print(f'warning: {msg}', file=sys.stderr)


def get_nix_dir() -> Path:
    """Get the path to bundled Nix files."""
    pkg_dir = Path(__file__).parent  # src/trix/

    # Development: nix/ at repo root (src/trix -> src -> repo_root)
    repo_root = pkg_dir.parent.parent
    dev_nix = repo_root / 'nix'
    if dev_nix.is_dir() and (dev_nix / 'eval.nix').exists():
        return dev_nix

    # Installed: $out/share/trix/nix/ (lib/python.../site-packages/trix -> ... -> $out)
    # Walk up to find share/trix/nix
    for parent in pkg_dir.parents:
        installed_nix = parent / 'share' / 'trix' / 'nix'
        if installed_nix.is_dir() and (installed_nix / 'eval.nix').exists():
            return installed_nix

    raise RuntimeError('Cannot find nix/ directory')


@lru_cache(maxsize=1)
def get_system() -> str:
    """Get the current Nix system (e.g., x86_64-linux). Result is cached."""
    result = subprocess.run(
        ['nix-instantiate', '--eval', '--expr', 'builtins.currentSystem', '--json'],
        capture_output=True,
        text=True,
        env=_get_clean_env(),
    )
    if result.returncode != 0:
        # Fallback
        import platform

        machine = platform.machine()
        system = platform.system().lower()
        return f'{machine}-{system}'
    else:
        return json.loads(result.stdout)


@lru_cache(maxsize=1)
def get_store_dir() -> str:
    """Get the Nix store directory (e.g., /nix/store). Result is cached."""
    result = subprocess.run(
        ['nix-instantiate', '--eval', '--expr', 'builtins.storeDir', '--json'],
        capture_output=True,
        text=True,
        env=_get_clean_env(),
    )
    if result.returncode != 0:
        # Fallback to default
        return '/nix/store'
    else:
        return json.loads(result.stdout)


def eval_expr(expr: str, cwd: Path | None = None) -> any:
    """Evaluate a Nix expression and return JSON result."""
    cmd = ['nix-instantiate', '--eval', '--expr', expr, '--json']
    result = subprocess.run(cmd, capture_output=True, text=True, cwd=cwd, env=_get_clean_env())
    if result.returncode != 0:
        raise RuntimeError(f'nix-instantiate failed: {result.stderr}')
    return json.loads(result.stdout)


def run_nix_build(
    flake_dir: Path,
    attr: str,
    out_link: str | None = 'result',
    verbose: bool = False,
    capture_output: bool = False,
    extra_args: list[tuple[str, str]] | None = None,
    extra_argstrs: list[tuple[str, str]] | None = None,
    store: str | None = None,
) -> str | None:
    """Run nix-build with eval.nix wrapper.

    Args:
        flake_dir: Directory containing flake.nix
        attr: Attribute path to build
        out_link: Name for result symlink (None for --no-link)
        verbose: Print commands
        capture_output: Return store path instead of None
        extra_args: List of (name, expr) tuples for --arg
        extra_argstrs: List of (name, value) tuples for --argstr
        store: Alternative nix store URL (e.g., "local?root=/tmp/store")

    Returns store path if capture_output=True, else None.
    """
    nix_dir = get_nix_dir()
    system = get_system()

    cmd = [
        'nix-build',
        str(nix_dir / 'eval.nix'),
        '--arg',
        'flakeDir',
        str(flake_dir),
        '--argstr',
        'system',
        system,
        '--argstr',
        'attr',
        attr,
    ]

    if store:
        cmd.extend(['--store', store])

    # Add extra --arg options
    if extra_args:
        for name, expr in extra_args:
            cmd.extend(['--arg', name, expr])

    # Add extra --argstr options
    if extra_argstrs:
        for name, value in extra_argstrs:
            cmd.extend(['--argstr', name, value])

    if out_link is None:
        cmd.append('--no-link')
    else:
        cmd.extend(['-o', out_link])

    if verbose:
        print(f'+ {" ".join(cmd)}', file=sys.stderr)

    if capture_output:
        result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
        if result.returncode != 0:
            print(result.stderr, file=sys.stderr)
            sys.exit(result.returncode)
        return result.stdout.strip()
    else:
        result = subprocess.run(cmd, env=_get_clean_env())
        if result.returncode != 0:
            sys.exit(result.returncode)
        return None


def run_nix_shell(
    flake_dir: Path,
    attr: str,
    command: str | None = None,
    verbose: bool = False,
    extra_args: list[tuple[str, str]] | None = None,
    extra_argstrs: list[tuple[str, str]] | None = None,
    store: str | None = None,
    bash_prompt: str | None = None,
    bash_prompt_prefix: str | None = None,
    bash_prompt_suffix: str | None = None,
) -> None:
    """Run nix-shell with eval.nix wrapper. Replaces current process.

    Args:
        flake_dir: Directory containing flake.nix
        attr: Attribute path to build
        command: Command to run in shell (optional)
        verbose: Print commands
        extra_args: List of (name, expr) tuples for --arg
        extra_argstrs: List of (name, value) tuples for --argstr
        store: Alternative nix store URL (e.g., "local?root=/tmp/store")
        bash_prompt: Custom PS1 prompt (from nixConfig.bash-prompt)
        bash_prompt_prefix: Prefix for prompt (from nixConfig.bash-prompt-prefix)
        bash_prompt_suffix: Suffix for prompt (from nixConfig.bash-prompt-suffix)
    """
    nix_dir = get_nix_dir()
    system = get_system()

    cmd = [
        'nix-shell',
        str(nix_dir / 'eval.nix'),
        '--arg',
        'flakeDir',
        str(flake_dir),
        '--argstr',
        'system',
        system,
        '--argstr',
        'attr',
        attr,
    ]

    if store:
        cmd.extend(['--store', store])

    # Add extra --arg options
    if extra_args:
        for name, expr in extra_args:
            cmd.extend(['--arg', name, expr])

    # Add extra --argstr options
    if extra_argstrs:
        for name, value in extra_argstrs:
            cmd.extend(['--argstr', name, value])

    if command:
        cmd.extend(['--command', command])

    if verbose:
        print(f'+ {" ".join(cmd)}', file=sys.stderr)

    # Set up environment with bash prompt if specified
    env = _get_clean_env()

    # Handle bash prompt settings (matching nix develop behavior)
    # We use PROMPT_COMMAND to set PS1 after bash initialization,
    # because .bashrc typically overwrites PS1
    if bash_prompt:
        # Escape single quotes for shell
        escaped_prompt = bash_prompt.replace("'", "'\\''")
        env['PROMPT_COMMAND'] = f"PS1='{escaped_prompt}'; unset PROMPT_COMMAND"
    elif bash_prompt_prefix or bash_prompt_suffix:
        prefix = bash_prompt_prefix or ''
        suffix = bash_prompt_suffix or ''
        default_prompt = '\\[\\e[0;1;35m\\][nix-shell:\\w]$\\[\\e[0m\\] '
        full_prompt = f'{prefix}{default_prompt}{suffix}'
        escaped_prompt = full_prompt.replace("'", "'\\''")
        env['PROMPT_COMMAND'] = f"PS1='{escaped_prompt}'; unset PROMPT_COMMAND"

    os.execvpe(cmd[0], cmd, env)


def run_nix_eval(
    flake_dir: Path | None,
    attr: str,
    output_json: bool = False,
    raw: bool = False,
    apply_fn: str | None = None,
    verbose: bool = False,
    extra_args: list[tuple[str, str]] | None = None,
    extra_argstrs: list[tuple[str, str]] | None = None,
    expr: str | None = None,
    store: str | None = None,
    quiet: bool = False,
) -> str:
    """Evaluate a flake attribute or raw expression and return the result.

    Args:
        flake_dir: Directory containing flake.nix (None if using expr)
        attr: Attribute path to evaluate (ignored if using expr)
        output_json: Output as JSON
        raw: Output raw string without quotes
        apply_fn: Nix function to apply to result
        verbose: Print commands
        extra_args: List of (name, expr) tuples for --arg
        extra_argstrs: List of (name, value) tuples for --argstr
        expr: Raw Nix expression to evaluate (bypasses flake evaluation)
        store: Alternative nix store URL (e.g., "local?root=/tmp/store")
        quiet: Suppress error output (for probing if attrs exist)

    Returns the evaluation result as a string.
    """
    if expr:
        # Raw expression evaluation
        if apply_fn:
            nix_expr = f'({apply_fn}) ({expr})'
        else:
            nix_expr = expr
    else:
        # Flake-based evaluation
        preamble = _flake_eval_preamble(flake_dir)
        attr_list = _attr_to_nix_list(attr)

        # Build the full expression
        # Uses fallback search matching nix eval behavior:
        # 1. packages.<system>.<attr>
        # 2. legacyPackages.<system>.<attr>
        # 3. <attr> (as-is)
        nix_expr = f'''
        let
          {preamble}
          userAttrPath = {attr_list};

          # Empty attr means "default" (matching nix behavior: .# -> .#default)
          effectiveAttrPath = if userAttrPath == [] then ["default"] else userAttrPath;

          # Paths to try in order (matching nix eval behavior)
          pathsToTry = [
            (["packages" system] ++ effectiveAttrPath)
            (["legacyPackages" system] ++ effectiveAttrPath)
            effectiveAttrPath
          ];

          validPaths = builtins.filter (p: hasPath p outputs) pathsToTry;

          value =
            if validPaths == []
            then throw "flake does not provide attribute '${{builtins.concatStringsSep "." (builtins.head pathsToTry)}}', '${{builtins.concatStringsSep "." (builtins.elemAt pathsToTry 1)}}' or '${{builtins.concatStringsSep "." userAttrPath}}'"
            else getPath (builtins.head validPaths) outputs;
        in {'(' + apply_fn + ') value' if apply_fn else 'value'}
        '''

    cmd = ['nix-instantiate', '--eval', '--expr', nix_expr, '--strict', '--read-write-mode']

    if store:
        cmd.extend(['--store', store])

    if output_json:
        cmd.append('--json')

    # Add extra --arg options
    if extra_args:
        for name, arg_expr in extra_args:
            cmd.extend(['--arg', name, arg_expr])

    # Add extra --argstr options
    if extra_argstrs:
        for name, value in extra_argstrs:
            cmd.extend(['--argstr', name, value])

    if verbose:
        what = 'expression' if expr else attr or 'outputs'
        print(f'+ nix-instantiate --eval ... (evaluating {what})', file=sys.stderr)

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        if not quiet:
            print(result.stderr, file=sys.stderr)
        sys.exit(result.returncode)

    output = result.stdout.strip()

    # Handle --raw: strip quotes from string output
    if raw and output.startswith('"') and output.endswith('"'):
        # Unescape the nix string
        output = output[1:-1].encode().decode('unicode_escape')

    return output


def flake_has_attr(flake_dir: Path, attr: str) -> bool:
    """Check if a flake has a specific attribute path.

    Args:
        flake_dir: Directory containing flake.nix
        attr: Attribute path to check (e.g., "devShells.x86_64-linux.default")

    Returns:
        True if the attribute exists, False otherwise.
    """
    preamble = _flake_eval_preamble(flake_dir)
    attr_list = _attr_to_nix_list(attr)

    nix_expr = f'''
    let
      {preamble}
      attrPath = {attr_list};
    in hasPath attrPath outputs
    '''

    cmd = ['nix-instantiate', '--eval', '--expr', nix_expr, '--read-write-mode']
    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())

    if result.returncode != 0:
        return False

    return result.stdout.strip() == 'true'


def run_nix_repl(
    flake_dir: Path,
    verbose: bool = False,
) -> None:
    """Run nix repl with flake context loaded. Replaces current process.

    Args:
        flake_dir: Directory containing flake.nix
        verbose: Print commands
    """
    nix_dir = get_nix_dir()
    system = get_system()

    cmd = [
        'nix',
        'repl',
        '--file',
        str(nix_dir / 'repl.nix'),
        '--arg',
        'flakeDir',
        str(flake_dir),
        '--argstr',
        'system',
        system,
    ]

    if verbose:
        print(f'+ {" ".join(cmd)}', file=sys.stderr)

    os.execvpe(cmd[0], cmd, _get_clean_env())


def get_derivation_path(
    flake_dir: Path,
    attr: str,
    verbose: bool = False,
) -> str:
    """Get the derivation path for a flake attribute without building.

    Uses nix-instantiate to evaluate the attribute and return the .drv path.

    Args:
        flake_dir: Directory containing flake.nix
        attr: Attribute path to evaluate
        verbose: Print commands

    Returns the .drv path (e.g., /nix/store/xxx.drv)
    """
    nix_dir = get_nix_dir()
    system = get_system()

    cmd = [
        'nix-instantiate',
        str(nix_dir / 'eval.nix'),
        '--arg',
        'flakeDir',
        str(flake_dir),
        '--argstr',
        'system',
        system,
        '--argstr',
        'attr',
        attr,
    ]

    if verbose:
        print(f'+ {" ".join(cmd)}', file=sys.stderr)

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        print(result.stderr, file=sys.stderr)
        sys.exit(result.returncode)

    return result.stdout.strip()


def get_store_path_from_drv(drv_path: str, verbose: bool = False) -> str:
    """Get the output store path from a derivation path.

    Args:
        drv_path: Path to the .drv file
        verbose: Print commands

    Returns the output store path
    """
    cmd = ['nix-store', '-q', '--outputs', drv_path]

    if verbose:
        print(f'+ {" ".join(cmd)}', file=sys.stderr)

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        print(result.stderr, file=sys.stderr)
        sys.exit(result.returncode)

    return result.stdout.strip().split('\n')[0]  # First output


def get_build_log(store_path: str, verbose: bool = False) -> str | None:
    """Get the build log for a store path.

    Args:
        store_path: Store path or derivation path
        verbose: Print commands

    Returns the build log, or None if not available
    """
    cmd = ['nix-store', '--read-log', store_path]

    if verbose:
        print(f'+ {" ".join(cmd)}', file=sys.stderr)

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        return None

    return result.stdout


def eval_flake_outputs(
    flake_dir: Path, verbose: bool = False, all_systems: bool = False, show_legacy: bool = False
) -> dict | None:
    """Get the structure of flake outputs.

    Returns a nested dict showing the output structure. For per-system outputs,
    includes all systems but only evaluates the current system to get derivation
    names. Non-current systems are marked with {"_omitted": true} unless
    all_systems=True, in which case all systems are fully evaluated.

    legacyPackages contents are not enumerated by default (they can be huge, e.g.
    all of nixpkgs). Set show_legacy=True to enumerate them.
    """
    preamble = _flake_eval_preamble(flake_dir)
    all_systems_nix = 'true' if all_systems else 'false'
    show_legacy_nix = 'true' if show_legacy else 'false'

    expr = f'''
    let
      {preamble}
      allSystemsFlag = {all_systems_nix};
      showLegacyFlag = {show_legacy_nix};

      # Standard per-system output categories (system to name to derivation)
      perSystemAttrs = [ "packages" "devShells" "checks" "apps" "legacyPackages" ];

      # Formatter is special: system to derivation (not system to name to derivation)
      formatterAttr = "formatter";

      # Module categories (enumerate names but mark as modules)
      moduleAttrs = [ "nixosModules" "darwinModules" "homeManagerModules" "flakeModules" ];

      # Template categories
      templateAttrs = [ "templates" ];

      # Get names at a level - just collect attr names without evaluating values
      # (Evaluating values to check if they're derivations forces thunk evaluation
      # which is very slow for large flakes like nixpkgs)
      getNames = attrs:
        builtins.listToAttrs (map (name: {{
          inherit name;
          value = null;  # Don't evaluate the value at all
        }}) (builtins.attrNames attrs));

      # Process output category based on its type
      processCategory = name: val:
        # Per-system categories (packages, devShells, etc.)
        if builtins.elem name perSystemAttrs && builtins.isAttrs val
        then
          if name == "legacyPackages" && !showLegacyFlag
          then
            # For legacyPackages, just mark systems as present but omit contents
            let allSystems = builtins.attrNames val;
            in builtins.listToAttrs (map (sys: {{
              name = sys;
              value = {{ _legacyOmitted = true; }};
            }}) allSystems)
          else
            let
              allSystems = builtins.attrNames val;
            in builtins.listToAttrs (map (sys: {{
              name = sys;
              value =
                if sys == system || allSystemsFlag
                then getNames val.${{sys}}
                else
                  # For non-current systems, just get attr names without evaluating
                  let sysVal = val.${{sys}}; in
                  if builtins.isAttrs sysVal
                  then builtins.listToAttrs (map (n: {{
                    name = n;
                    value = {{ _omitted = true; }};
                  }}) (builtins.attrNames sysVal))
                  else {{ _omitted = true; }};
            }}) allSystems)

        # Formatter: system maps to derivation directly (not system to name to derivation)
        else if name == formatterAttr && builtins.isAttrs val
        then
          let allSystems = builtins.attrNames val;
          in builtins.listToAttrs (map (sys: {{
            name = sys;
            value =
              if sys == system || allSystemsFlag
              then {{ _type = "formatter"; }}  # Current system - would show derivation info
              else {{ _omitted = true; }};  # Non-current system
          }}) allSystems)

        # Module categories (nixosModules, etc.) - enumerate but mark as modules
        else if builtins.elem name moduleAttrs && builtins.isAttrs val
        then builtins.listToAttrs (map (n: {{
          name = n;
          value = {{ _type = "module"; }};
        }}) (builtins.attrNames val))

        # Template categories - enumerate but mark as templates
        else if builtins.elem name templateAttrs && builtins.isAttrs val
        then builtins.listToAttrs (map (n: {{
          name = n;
          value = {{ _type = "template"; }};
        }}) (builtins.attrNames val))

        # Overlays - just enumerate names
        else if name == "overlays" && builtins.isAttrs val
        then builtins.listToAttrs (map (n: {{
          name = n;
          value = {{ _type = "overlay"; }};
        }}) (builtins.attrNames val))

        # NixOS/Darwin/Home configurations - enumerate names
        else if (name == "nixosConfigurations" || name == "darwinConfigurations" || name == "homeConfigurations") && builtins.isAttrs val
        then builtins.listToAttrs (map (n: {{
          name = n;
          value = {{ _type = "configuration"; }};
        }}) (builtins.attrNames val))

        # Everything else (lib, htmlDocs, etc.) - mark as unknown, don't enumerate
        else {{ _unknown = true; }};

    in builtins.mapAttrs processCategory outputs
    '''

    cmd = ['nix-instantiate', '--eval', '--expr', expr, '--json', '--strict', '--read-write-mode']

    if verbose:
        print('+ nix-instantiate --eval ... (getting outputs structure)', file=sys.stderr)

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        if verbose:
            print(result.stderr, file=sys.stderr)
        return None

    return json.loads(result.stdout)


def get_flake_output_categories(flake_dir: Path, verbose: bool = False) -> list[str] | None:
    """Get the list of top-level output category names.

    This is a quick evaluation that just gets attribute names without
    evaluating the contents. Used to parallelize category evaluation.
    """
    preamble = _flake_eval_preamble(flake_dir)

    expr = f'''
    let
      {preamble}
    in builtins.attrNames outputs
    '''

    cmd = ['nix-instantiate', '--eval', '--expr', expr, '--json', '--read-write-mode']

    if verbose:
        print('+ nix-instantiate --eval ... (getting output categories)', file=sys.stderr)

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        if verbose:
            print(result.stderr, file=sys.stderr)
        return None

    return json.loads(result.stdout)


def eval_flake_output_category(
    flake_dir: Path,
    category: str,
    verbose: bool = False,
    all_systems: bool = False,
    show_legacy: bool = False,
) -> tuple[str, dict | None]:
    """Evaluate a single output category's structure.

    Returns (category_name, structure_dict) tuple.
    This is designed to be called in parallel for each category.
    """
    preamble = _flake_eval_preamble(flake_dir)
    all_systems_nix = 'true' if all_systems else 'false'
    show_legacy_nix = 'true' if show_legacy else 'false'

    expr = f'''
    let
      {preamble}
      allSystemsFlag = {all_systems_nix};
      showLegacyFlag = {show_legacy_nix};
      categoryName = "{category}";

      # Standard per-system output categories (system to name to derivation)
      perSystemAttrs = [ "packages" "devShells" "checks" "apps" "legacyPackages" ];

      # Formatter is special: system to derivation (not system to name to derivation)
      formatterAttr = "formatter";

      # Module categories (enumerate names but mark as modules)
      moduleAttrs = [ "nixosModules" "darwinModules" "homeManagerModules" "flakeModules" ];

      # Template categories
      templateAttrs = [ "templates" ];

      # Get names at a level - just collect attr names without evaluating values
      # (Evaluating values to check if they're derivations forces thunk evaluation
      # which is very slow for large flakes like nixpkgs)
      getNames = attrs:
        builtins.listToAttrs (map (name: {{
          inherit name;
          value = null;  # Don't evaluate the value at all
        }}) (builtins.attrNames attrs));

      # Process output category based on its type
      processCategory = name: val:
        # Per-system categories (packages, devShells, etc.)
        if builtins.elem name perSystemAttrs && builtins.isAttrs val
        then
          if name == "legacyPackages" && !showLegacyFlag
          then
            let allSystems = builtins.attrNames val;
            in builtins.listToAttrs (map (sys: {{
              name = sys;
              value = {{ _legacyOmitted = true; }};
            }}) allSystems)
          else
            let
              allSystems = builtins.attrNames val;
            in builtins.listToAttrs (map (sys: {{
              name = sys;
              value =
                if sys == system || allSystemsFlag
                then getNames val.${{sys}}
                else
                  let sysVal = val.${{sys}}; in
                  if builtins.isAttrs sysVal
                  then builtins.listToAttrs (map (n: {{
                    name = n;
                    value = {{ _omitted = true; }};
                  }}) (builtins.attrNames sysVal))
                  else {{ _omitted = true; }};
            }}) allSystems)

        # Formatter: system maps to derivation directly (not system to name to derivation)
        else if name == formatterAttr && builtins.isAttrs val
        then
          let allSystems = builtins.attrNames val;
          in builtins.listToAttrs (map (sys: {{
            name = sys;
            value =
              if sys == system || allSystemsFlag
              then {{ _type = "formatter"; }}  # Current system - would show derivation info
              else {{ _omitted = true; }};  # Non-current system
          }}) allSystems)

        # Module categories (nixosModules, etc.) - enumerate but mark as modules
        else if builtins.elem name moduleAttrs && builtins.isAttrs val
        then builtins.listToAttrs (map (n: {{
          name = n;
          value = {{ _type = "module"; }};
        }}) (builtins.attrNames val))

        # Template categories - enumerate but mark as templates
        else if builtins.elem name templateAttrs && builtins.isAttrs val
        then builtins.listToAttrs (map (n: {{
          name = n;
          value = {{ _type = "template"; }};
        }}) (builtins.attrNames val))

        # Overlays - just enumerate names
        else if name == "overlays" && builtins.isAttrs val
        then builtins.listToAttrs (map (n: {{
          name = n;
          value = {{ _type = "overlay"; }};
        }}) (builtins.attrNames val))

        # NixOS/Darwin/Home configurations - enumerate names
        else if (name == "nixosConfigurations" || name == "darwinConfigurations" || name == "homeConfigurations") && builtins.isAttrs val
        then builtins.listToAttrs (map (n: {{
          name = n;
          value = {{ _type = "configuration"; }};
        }}) (builtins.attrNames val))

        # Everything else (lib, htmlDocs, etc.) - mark as unknown, don't enumerate
        else {{ _unknown = true; }};

      val = outputs.${{categoryName}};
    in processCategory categoryName val
    '''

    cmd = ['nix-instantiate', '--eval', '--expr', expr, '--json', '--strict', '--read-write-mode']

    result = subprocess.run(cmd, capture_output=True, text=True, env=_get_clean_env())
    if result.returncode != 0:
        if verbose:
            print(f'Failed to evaluate {category}: {result.stderr}', file=sys.stderr)
        return (category, None)

    return (category, json.loads(result.stdout))


def eval_flake_outputs_parallel(
    flake_dir: Path,
    verbose: bool = False,
    all_systems: bool = False,
    show_legacy: bool = False,
    jobs: int = 4,
) -> dict | None:
    """Get the structure of flake outputs using parallel evaluation.

    Evaluates each output category in parallel for faster results.
    Results are returned in sorted order regardless of completion order.
    """
    from concurrent.futures import ThreadPoolExecutor, as_completed

    # First, get the list of categories (quick)
    categories = get_flake_output_categories(flake_dir, verbose=verbose)
    if categories is None:
        return None

    if not categories:
        return {}

    if verbose:
        print(f'+ Evaluating {len(categories)} categories in parallel (jobs={jobs})', file=sys.stderr)

    # Evaluate each category in parallel
    results = {}
    with ThreadPoolExecutor(max_workers=jobs) as executor:
        futures = {
            executor.submit(
                eval_flake_output_category,
                flake_dir,
                cat,
                verbose=False,  # Don't print per-category, too noisy
                all_systems=all_systems,
                show_legacy=show_legacy,
            ): cat
            for cat in categories
        }

        for future in as_completed(futures):
            cat_name, cat_result = future.result()
            if cat_result is not None:
                results[cat_name] = cat_result

    return results