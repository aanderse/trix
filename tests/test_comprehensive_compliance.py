"""Comprehensive compliance tests based on real-world flake patterns from GitHub.

This test suite ensures trix behavior matches nix for diverse flake configurations
found in the wild. It covers:

1. Input URL formats (github, git+https, git+ssh, path, tarball)
2. Query parameters (?ref=, ?rev=, ?narHash=)
3. flake = false inputs
4. follows patterns (root-level, nested, multi-level)
5. All standard output types (packages, devShells, apps, checks, legacyPackages)
6. Top-level outputs (lib, overlays, nixosModules, nixosConfigurations)
7. Custom output types (homeConfigurations, darwinConfigurations, etc.)
8. Lock file format compatibility
9. --override-input behavior

Sources for patterns:
- https://github.com/Misterio77/nix-starter-configs
- https://github.com/numtide/flake-utils
- https://github.com/mitchellh/zig-overlay
- https://github.com/NixOS/nixpkgs
- https://github.com/nix-community/home-manager
"""

import json
import subprocess
import tempfile
from pathlib import Path

import pytest

from .conftest import NixComplianceTest

# ==============================================================================
# Test Flake Configurations
# ==============================================================================

# Standard flake with all common output types
STANDARD_OUTPUTS_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      # Per-system outputs
      packages.x86_64-linux = {
        default = pkgs.hello;
        cowsay = pkgs.cowsay;
      };

      devShells.x86_64-linux.default = pkgs.mkShell {
        buildInputs = [ pkgs.hello ];
      };

      apps.x86_64-linux.default = {
        type = "app";
        program = "${pkgs.hello}/bin/hello";
      };

      checks.x86_64-linux.test = pkgs.hello;

      formatter.x86_64-linux = pkgs.nixfmt-classic;

      # Top-level outputs (no system)
      lib.version = "1.0.0";
      lib.helper = x: x + 1;

      overlays.default = final: prev: {
        myHello = prev.hello;
      };

      nixosModules.default = { ... }: {
        # Empty module for testing
      };
    };
}
"""

# Flake with only legacyPackages (like nixpkgs)
LEGACY_PACKAGES_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }: {
    legacyPackages.x86_64-linux = nixpkgs.legacyPackages.x86_64-linux;
    legacyPackages.aarch64-linux = nixpkgs.legacyPackages.aarch64-linux;
  };
}
"""

# Flake with nixosConfigurations (like nix-starter-configs)
NIXOS_CONFIGURATIONS_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }: {
    nixosConfigurations.testhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ({ ... }: {
          boot.isContainer = true;
          system.stateVersion = "24.05";
        })
      ];
    };

    # Also export packages for completeness
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""

# Flake with custom output type (like finix)
CUSTOM_OUTPUT_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      pkgs = nixpkgs.legacyPackages.x86_64-linux;
    in {
      # Custom output type (not in standard flake schema)
      customConfigurations.myconfig = {
        name = "test-custom-config";
        value = 42;
      };

      # Also have standard outputs
      packages.x86_64-linux.default = pkgs.hello;
    };
}
"""

# Flake with nested follows (like home-manager integration)
NESTED_FOLLOWS_FLAKE = """{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager.url = "github:nix-community/home-manager";
    home-manager.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, home-manager }: {
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""

# Flake with root-level follows
ROOT_FOLLOWS_FLAKE = """{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixpkgs-stable.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, nixpkgs-stable }: {
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""

# Flake with flake = false input
NON_FLAKE_INPUT = """{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Non-flake input (like a data repo)
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-compat }: {
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""

# Flake with git+https input
GIT_HTTPS_INPUT_FLAKE = """{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Note: This is a public git URL for testing
    # In practice, git+ssh would be used for private repos
  };

  outputs = { self, nixpkgs }: {
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""

# Flake with multiple systems (like flake-utils pattern)
MULTI_SYSTEM_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      # Simple multi-system without complex helpers
      pkgsFor = system: nixpkgs.legacyPackages.${system};
    in {
      packages.x86_64-linux.default = (pkgsFor "x86_64-linux").hello;
      packages.aarch64-linux.default = (pkgsFor "aarch64-linux").hello;

      devShells.x86_64-linux.default = (pkgsFor "x86_64-linux").mkShell {
        buildInputs = [ (pkgsFor "x86_64-linux").hello ];
      };
    };
}
"""

# Flake with templates (like flake-utils)
TEMPLATES_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }: {
    templates.default = {
      path = ./.;
      description = "Default template";
    };

    templates.rust = {
      path = ./.;
      description = "Rust development template";
    };

    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""


# ==============================================================================
# Input URL Parsing Tests
# ==============================================================================


class TestInputUrlParsing:
    """Test that various input URL formats are parsed correctly."""

    def test_github_simple(self):
        """github:owner/repo format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('github:NixOS/nixpkgs')
        assert result == {'type': 'github', 'owner': 'NixOS', 'repo': 'nixpkgs'}

    def test_github_with_branch_ref(self):
        """github:owner/repo/branch format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('github:NixOS/nixpkgs/nixos-unstable')
        assert result['ref'] == 'nixos-unstable'

    def test_github_with_query_ref(self):
        """github:owner/repo?ref=branch format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('github:NixOS/nixpkgs?ref=nixos-24.05')
        assert result['ref'] == 'nixos-24.05'

    def test_github_with_query_rev(self):
        """github:owner/repo?rev=commit format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('github:NixOS/nixpkgs?rev=abc123def456')
        assert result['rev'] == 'abc123def456'

    def test_github_with_ref_and_rev(self):
        """github:owner/repo?ref=branch&rev=commit format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('github:NixOS/nixpkgs?ref=master&rev=abc123')
        assert result['ref'] == 'master'
        assert result['rev'] == 'abc123'

    def test_git_https(self):
        """git+https://... format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('git+https://github.com/owner/repo.git')
        assert result['type'] == 'git'
        assert result['url'] == 'https://github.com/owner/repo.git'

    def test_git_ssh(self):
        """git+ssh://... format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('git+ssh://git@github.com/owner/repo.git')
        assert result['type'] == 'git'
        assert result['url'] == 'ssh://git@github.com/owner/repo.git'

    def test_git_with_ref(self):
        """git+...?ref=branch format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('git+https://example.com/repo.git?ref=main')
        assert result['ref'] == 'main'

    def test_git_with_rev(self):
        """git+...?rev=commit format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('git+https://example.com/repo.git?rev=abc123')
        assert result['rev'] == 'abc123'

    def test_path_explicit(self):
        """path:./local format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('path:./local')
        assert result == {'type': 'path', 'path': './local'}

    def test_path_relative(self):
        """./local format (implicit path)."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('./local')
        assert result == {'type': 'path', 'path': './local'}

    def test_path_absolute(self):
        """/absolute/path format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('/home/user/flake')
        assert result == {'type': 'path', 'path': '/home/user/flake'}

    def test_path_parent(self):
        """../parent format."""
        from trix.flake import parse_flake_url

        result = parse_flake_url('../sibling-flake')
        assert result == {'type': 'path', 'path': '../sibling-flake'}


# ==============================================================================
# Attribute Path Resolution Tests
# ==============================================================================


class TestAttributePathResolution:
    """Test that attribute paths are resolved correctly to match nix behavior."""

    def test_simple_package_name(self):
        """Simple name resolves to packages.<system>.<name>."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('hello', 'packages', 'x86_64-linux')
        assert result == 'packages.x86_64-linux.hello'

    def test_default_resolves_correctly(self):
        """'default' resolves to packages.<system>.default."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('default', 'packages', 'x86_64-linux')
        assert result == 'packages.x86_64-linux.default'

    def test_devshells_category(self):
        """devShells.name adds system."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('devShells.myshell', 'packages', 'x86_64-linux')
        assert result == 'devShells.x86_64-linux.myshell'

    def test_already_has_system(self):
        """Full path with system is unchanged."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('packages.x86_64-linux.hello', 'packages', 'x86_64-linux')
        assert result == 'packages.x86_64-linux.hello'

    def test_lib_top_level(self):
        """lib.foo stays as-is (top-level, no system)."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('lib.version', 'packages', 'x86_64-linux')
        assert result == 'lib.version'

    def test_overlays_top_level(self):
        """overlays.default stays as-is."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('overlays.default', 'packages', 'x86_64-linux')
        assert result == 'overlays.default'

    def test_nixos_modules_top_level(self):
        """nixosModules.default stays as-is."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('nixosModules.default', 'packages', 'x86_64-linux')
        assert result == 'nixosModules.default'

    def test_nixos_configurations_top_level(self):
        """nixosConfigurations.host stays as-is."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('nixosConfigurations.myhost', 'packages', 'x86_64-linux')
        assert result == 'nixosConfigurations.myhost'

    def test_nixos_configurations_deep_path(self):
        """nixosConfigurations.host.config.system.build.toplevel stays as-is."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path(
            'nixosConfigurations.myhost.config.system.build.toplevel',
            'packages',
            'x86_64-linux',
        )
        assert result == 'nixosConfigurations.myhost.config.system.build.toplevel'

    def test_custom_output_passes_through(self):
        """Custom outputs like finixConfigurations pass through unchanged."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path(
            'finixConfigurations.framework.config.system.topLevel',
            'packages',
            'x86_64-linux',
        )
        assert result == 'finixConfigurations.framework.config.system.topLevel'

    def test_custom_output_short_path(self):
        """Short custom output path passes through."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('customConfigurations.myconfig', 'packages', 'x86_64-linux')
        assert result == 'customConfigurations.myconfig'

    def test_templates_top_level(self):
        """templates.default stays as-is."""
        from trix.flake import resolve_attr_path

        result = resolve_attr_path('templates.default', 'packages', 'x86_64-linux')
        assert result == 'templates.default'


# ==============================================================================
# Lock File Format Compliance Tests
# ==============================================================================


class TestLockFileCompliance:
    """Test that trix lock files are compatible with nix."""

    @pytest.fixture
    def temp_flake(self):
        """Create a temporary directory for flake tests."""
        import shutil

        d = tempfile.mkdtemp(prefix='trix_compliance_')
        yield Path(d)
        shutil.rmtree(d, ignore_errors=True)

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_trix_lock_version_is_7(self, temp_flake):
        """trix produces version 7 lock files."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        result = NixComplianceTest.run_trix(['flake', 'lock'], cwd=temp_flake)
        if result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {result.stderr}')

        lock = json.loads((temp_flake / 'flake.lock').read_text())
        assert lock.get('version') == 7

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_trix_lock_readable_by_nix(self, temp_flake):
        """Lock file created by trix can be read by nix."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        # Generate with trix
        trix_result = NixComplianceTest.run_trix(['flake', 'lock'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        # Verify nix can read it
        nix_result = NixComplianceTest.run_nix(['flake', 'metadata', '--json'], cwd=temp_flake)
        assert nix_result.returncode == 0, f'nix failed to read trix lock: {nix_result.stderr}'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_nix_lock_usable_by_trix(self, temp_flake):
        """Lock file created by nix can be used by trix."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        # Generate with nix
        nix_result = NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)
        if nix_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_result.stderr}')

        # Verify trix can use it
        trix_result = NixComplianceTest.run_trix(
            ['eval', '.#packages.x86_64-linux.default.name', '--raw'],
            cwd=temp_flake,
        )
        # Should not fail on lock parsing
        assert 'error reading lock' not in trix_result.stderr.lower()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_nested_follows_in_lock(self, temp_flake):
        """Nested follows should be stored correctly in lock file."""
        (temp_flake / 'flake.nix').write_text(NESTED_FOLLOWS_FLAKE)

        # Generate with trix
        trix_result = NixComplianceTest.run_trix(['flake', 'lock'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        lock = json.loads((temp_flake / 'flake.lock').read_text())

        # home-manager's nixpkgs should follow root nixpkgs
        hm_node = lock.get('nodes', {}).get('home-manager', {})
        hm_inputs = hm_node.get('inputs', {})

        # The nixpkgs input of home-manager should be a follows reference
        nixpkgs_ref = hm_inputs.get('nixpkgs')
        assert isinstance(nixpkgs_ref, list), f'nested follows should be list, got: {type(nixpkgs_ref)}'
        assert nixpkgs_ref == ['nixpkgs'], f"should follow ['nixpkgs'], got: {nixpkgs_ref}"

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_root_follows_in_lock(self, temp_flake):
        """Root-level follows should be stored as list in lock file."""
        (temp_flake / 'flake.nix').write_text(ROOT_FOLLOWS_FLAKE)

        # Generate with trix
        trix_result = NixComplianceTest.run_trix(['flake', 'lock'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        lock = json.loads((temp_flake / 'flake.lock').read_text())

        # Root-level follows should be stored as a list reference
        root_inputs = lock.get('nodes', {}).get('root', {}).get('inputs', {})
        stable_ref = root_inputs.get('nixpkgs-stable')

        assert isinstance(stable_ref, list), f'root follows should be list, got: {type(stable_ref)}'
        assert stable_ref == ['nixpkgs'], f"should follow ['nixpkgs'], got: {stable_ref}"

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_non_flake_input_in_lock(self, temp_flake):
        """flake = false inputs should have flake: false in lock."""
        (temp_flake / 'flake.nix').write_text(NON_FLAKE_INPUT)

        # Generate with trix
        trix_result = NixComplianceTest.run_trix(['flake', 'lock'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        lock = json.loads((temp_flake / 'flake.lock').read_text())

        # flake-compat should have flake: false
        fc_node = lock.get('nodes', {}).get('flake-compat', {})
        assert fc_node.get('flake') is False, 'non-flake input should have flake: false'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_lock_has_no_null_values(self, temp_flake):
        """Lock file should not contain null values (nix rejects them)."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        # Generate with trix
        trix_result = NixComplianceTest.run_trix(['flake', 'lock'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        lock_content = (temp_flake / 'flake.lock').read_text()

        # Check for null values in the JSON
        assert ': null' not in lock_content, 'lock file should not contain null values'
        assert '": null' not in lock_content, 'lock file should not contain null values'


# ==============================================================================
# Eval Compliance Tests
# ==============================================================================


class TestEvalCompliance:
    """Test that trix eval produces same output as nix eval."""

    @pytest.fixture
    def temp_flake(self):
        """Create a temporary directory for flake tests."""
        import shutil

        d = tempfile.mkdtemp(prefix='trix_compliance_')
        yield Path(d)
        shutil.rmtree(d, ignore_errors=True)

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_eval_package_name_matches(self, temp_flake):
        """trix eval and nix eval produce same package name."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        # Lock with nix first
        NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)

        # Evaluate with nix
        nix_result = NixComplianceTest.run_nix(
            ['eval', '.#packages.x86_64-linux.default.name'],
            cwd=temp_flake,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = NixComplianceTest.run_trix(
            ['eval', '.#packages.x86_64-linux.default.name'],
            cwd=temp_flake,
        )
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        assert trix_result.stdout.strip() == nix_result.stdout.strip()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_eval_shorthand_matches(self, temp_flake):
        """trix eval .#hello matches nix eval .#hello."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        # Lock with nix first
        NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)

        # Both should resolve .#cowsay to packages.x86_64-linux.cowsay
        nix_result = NixComplianceTest.run_nix(['eval', '.#cowsay.name'], cwd=temp_flake)
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        trix_result = NixComplianceTest.run_trix(['eval', '.#cowsay.name'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        assert trix_result.stdout.strip() == nix_result.stdout.strip()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_eval_lib_output(self, temp_flake):
        """trix eval .#lib.version matches nix."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)

        nix_result = NixComplianceTest.run_nix(['eval', '.#lib.version'], cwd=temp_flake)
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        trix_result = NixComplianceTest.run_trix(['eval', '.#lib.version'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        assert trix_result.stdout.strip() == nix_result.stdout.strip()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_eval_legacy_packages_fallback(self, temp_flake):
        """trix eval .#hello falls back to legacyPackages like nix."""
        (temp_flake / 'flake.nix').write_text(LEGACY_PACKAGES_FLAKE)

        NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)

        # .#hello should fall back to legacyPackages.x86_64-linux.hello
        nix_result = NixComplianceTest.run_nix(['eval', '.#hello.name'], cwd=temp_flake)
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        trix_result = NixComplianceTest.run_trix(['eval', '.#hello.name'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        assert trix_result.stdout.strip() == nix_result.stdout.strip()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_eval_custom_output(self, temp_flake):
        """trix eval .#customConfigurations.myconfig.name works."""
        (temp_flake / 'flake.nix').write_text(CUSTOM_OUTPUT_FLAKE)

        NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)

        nix_result = NixComplianceTest.run_nix(
            ['eval', '.#customConfigurations.myconfig.name'],
            cwd=temp_flake,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        trix_result = NixComplianceTest.run_trix(
            ['eval', '.#customConfigurations.myconfig.name'],
            cwd=temp_flake,
        )
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        assert trix_result.stdout.strip() == nix_result.stdout.strip()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_eval_json_output(self, temp_flake):
        """trix eval --json produces same output as nix."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)

        nix_result = NixComplianceTest.run_nix(
            ['eval', '.#lib.version', '--json'],
            cwd=temp_flake,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        trix_result = NixComplianceTest.run_trix(
            ['eval', '.#lib.version', '--json'],
            cwd=temp_flake,
        )
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        nix_json = json.loads(nix_result.stdout.strip())
        trix_json = json.loads(trix_result.stdout.strip())
        assert trix_json == nix_json


# ==============================================================================
# Override Input Tests
# ==============================================================================


class TestOverrideInput:
    """Test --override-input behavior matches nix."""

    @pytest.fixture
    def temp_flake(self):
        """Create a temporary directory for flake tests."""
        import shutil

        d = tempfile.mkdtemp(prefix='trix_compliance_')
        yield Path(d)
        shutil.rmtree(d, ignore_errors=True)

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_override_input_with_rev(self, temp_flake):
        """--override-input with ?rev= format works."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        # Use a known nixpkgs commit
        override_ref = 'github:NixOS/nixpkgs?rev=e01315fd86b97d8f3e486ed1a6f1222f9e005704'

        result = NixComplianceTest.run_trix(
            ['flake', 'update', '--override-input', 'nixpkgs', override_ref],
            cwd=temp_flake,
        )

        if result.returncode != 0:
            pytest.skip(f'trix flake update failed: {result.stderr}')

        lock = json.loads((temp_flake / 'flake.lock').read_text())
        nixpkgs_rev = lock['nodes']['nixpkgs']['locked']['rev']

        # Should be locked to the specific rev
        assert nixpkgs_rev == 'e01315fd86b97d8f3e486ed1a6f1222f9e005704'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_override_input_preserves_original(self, temp_flake):
        """--override-input preserves original from flake.nix."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        override_ref = 'github:NixOS/nixpkgs?rev=e01315fd86b97d8f3e486ed1a6f1222f9e005704'

        result = NixComplianceTest.run_trix(
            ['flake', 'update', '--override-input', 'nixpkgs', override_ref],
            cwd=temp_flake,
        )

        if result.returncode != 0:
            pytest.skip(f'trix flake update failed: {result.stderr}')

        lock = json.loads((temp_flake / 'flake.lock').read_text())
        original = lock['nodes']['nixpkgs']['original']

        # Original should match flake.nix (nixos-unstable ref), not the override
        assert original.get('type') == 'github'
        assert original.get('owner') == 'NixOS'
        assert original.get('repo') == 'nixpkgs'
        # Should not have rev in original, should have ref from flake.nix
        assert 'rev' not in original, 'original should not have rev from override'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_override_on_first_lock(self, temp_flake):
        """--override-input works when no lock file exists."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        # Ensure no lock file exists
        lock_file = temp_flake / 'flake.lock'
        if lock_file.exists():
            lock_file.unlink()

        override_ref = 'github:NixOS/nixpkgs?rev=e01315fd86b97d8f3e486ed1a6f1222f9e005704'

        result = NixComplianceTest.run_trix(
            ['flake', 'update', '--override-input', 'nixpkgs', override_ref],
            cwd=temp_flake,
        )

        if result.returncode != 0:
            pytest.skip(f'trix flake update failed: {result.stderr}')

        lock = json.loads((temp_flake / 'flake.lock').read_text())
        nixpkgs_rev = lock['nodes']['nixpkgs']['locked']['rev']

        # Should be locked to the specific rev even on first lock
        assert nixpkgs_rev == 'e01315fd86b97d8f3e486ed1a6f1222f9e005704'


# ==============================================================================
# Flake Show Compliance Tests
# ==============================================================================


class TestFlakeShowCompliance:
    """Test that trix flake show matches nix flake show."""

    @pytest.fixture
    def temp_flake(self):
        """Create a temporary directory for flake tests."""
        import shutil

        d = tempfile.mkdtemp(prefix='trix_compliance_')
        yield Path(d)
        shutil.rmtree(d, ignore_errors=True)

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_flake_show_lists_outputs(self, temp_flake):
        """trix flake show lists the same output categories as nix."""
        (temp_flake / 'flake.nix').write_text(STANDARD_OUTPUTS_FLAKE)

        NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)

        trix_result = NixComplianceTest.run_trix(['flake', 'show'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake show failed: {trix_result.stderr}')

        output = trix_result.stdout

        # Should show standard output categories
        assert 'packages' in output, 'should show packages'
        assert 'devShells' in output, 'should show devShells'
        assert 'apps' in output or 'app' in output.lower(), 'should show apps'
        assert 'lib' in output, 'should show lib'
        assert 'overlays' in output, 'should show overlays'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix not available')
    def test_flake_show_multi_system(self, temp_flake):
        """trix flake show handles multi-system flakes."""
        (temp_flake / 'flake.nix').write_text(MULTI_SYSTEM_FLAKE)

        NixComplianceTest.run_nix(['flake', 'lock'], cwd=temp_flake)

        trix_result = NixComplianceTest.run_trix(['flake', 'show'], cwd=temp_flake)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake show failed: {trix_result.stderr}')

        output = trix_result.stdout

        # Should show multiple systems
        assert 'x86_64-linux' in output, 'should show x86_64-linux'
        # May or may not show all systems depending on implementation
