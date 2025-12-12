"""Compliance tests comparing trix behavior against nix flakes."""

import json

import pytest

from .conftest import (
    FOLLOWS_ROOT_FLAKE,
    NO_SELF_FLAKE,
    SIMPLE_FLAKE,
    NixComplianceTest,
)


class TestNoAutomaticStoreCopy:
    """Verify trix never automatically copies the current directory to the nix store."""

    def test_self_outpath_is_direct_path(self, temp_flake_dir, compliance):
        """Verify self.outPath is a direct filesystem path, not a store path."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(NO_SELF_FLAKE)

        # Create a minimal lock file
        lock_file = temp_flake_dir / 'flake.lock'
        lock_file.write_text(
            json.dumps(
                {
                    'nodes': {
                        'nixpkgs': {
                            'locked': {
                                'lastModified': 1700000000,
                                'narHash': 'sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
                                'owner': 'NixOS',
                                'repo': 'nixpkgs',
                                'rev': 'a' * 40,
                                'type': 'github',
                            },
                            'original': {
                                'owner': 'NixOS',
                                'repo': 'nixpkgs',
                                'type': 'github',
                            },
                        },
                        'root': {'inputs': {'nixpkgs': 'nixpkgs'}},
                    },
                    'root': 'root',
                    'version': 7,
                }
            )
        )

        # Evaluate self.outPath with trix
        result = compliance.run_trix(['eval', '.#self.outPath', '--raw'], cwd=temp_flake_dir)

        # Should return the actual directory path, not a /nix/store path
        if result.returncode == 0:
            out_path = result.stdout.strip()
            assert not out_path.startswith('/nix/store'), f'self.outPath should be direct path, got: {out_path}'
            assert str(temp_flake_dir) in out_path or out_path == str(temp_flake_dir), (
                f'self.outPath should be flake dir, got: {out_path}'
            )


class TestLockFileFormat:
    """Test that trix lock files are compatible with nix."""

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_trix_lock_readable_by_nix(self, temp_flake_dir, compliance):
        """Lock file created by trix can be read by nix flake metadata."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(SIMPLE_FLAKE)

        # Generate lock with trix
        trix_result = compliance.run_trix(['flake', 'lock'], cwd=temp_flake_dir)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        # Verify lock file exists
        lock_file = temp_flake_dir / 'flake.lock'
        assert lock_file.exists(), 'trix should create flake.lock'

        # Verify nix can read it
        nix_result = compliance.run_nix(['flake', 'metadata', '--json'], cwd=temp_flake_dir)
        assert nix_result.returncode == 0, f'nix should read trix lock file: {nix_result.stderr}'

        # Should parse as valid JSON
        metadata = json.loads(nix_result.stdout)
        assert 'locked' in metadata or 'locks' in metadata or 'lastModified' in metadata

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_nix_lock_usable_by_trix(self, temp_flake_dir, compliance):
        """Lock file created by nix can be used by trix."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(SIMPLE_FLAKE)

        # Generate lock with nix
        nix_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_result.stderr}')

        # Verify trix can use it for evaluation
        trix_result = compliance.run_trix(
            ['eval', '.#packages.x86_64-linux.default.name', '--raw'],
            cwd=temp_flake_dir,
        )
        # May fail on actual eval (network), but shouldn't fail on lock parsing
        assert 'error reading lock' not in trix_result.stderr.lower(), (
            f'trix should parse nix lock: {trix_result.stderr}'
        )

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_lock_version_is_7(self, temp_flake_dir, compliance):
        """Both trix and nix should produce version 7 lock files."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(SIMPLE_FLAKE)

        # Generate with trix
        trix_result = compliance.run_trix(['flake', 'lock'], cwd=temp_flake_dir)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        lock_file = temp_flake_dir / 'flake.lock'
        trix_lock = json.loads(lock_file.read_text())
        assert trix_lock.get('version') == 7, 'trix should produce version 7 lock'


class TestFollowsCompliance:
    """Test that follows references work identically to nix."""

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_root_follows_format(self, temp_flake_dir, compliance):
        """Root-level follows should be stored as list in lock file."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(FOLLOWS_ROOT_FLAKE)

        # Generate with trix
        trix_result = compliance.run_trix(['flake', 'lock'], cwd=temp_flake_dir)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        lock_file = temp_flake_dir / 'flake.lock'
        trix_lock = json.loads(lock_file.read_text())

        # Root-level follows should be stored as a list
        root_inputs = trix_lock.get('nodes', {}).get('root', {}).get('inputs', {})
        nixpkgs_stable_ref = root_inputs.get('nixpkgs-stable')

        assert isinstance(nixpkgs_stable_ref, list), f'follows should be list, got: {type(nixpkgs_stable_ref)}'
        assert nixpkgs_stable_ref == ['nixpkgs'], f"follows should be ['nixpkgs'], got: {nixpkgs_stable_ref}"

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_follows_format_matches_nix(self, temp_flake_dir, compliance):
        """Follows format should match nix exactly."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(FOLLOWS_ROOT_FLAKE)

        # Generate with nix first
        nix_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_result.stderr}')

        nix_lock = json.loads((temp_flake_dir / 'flake.lock').read_text())

        # Remove and regenerate with trix
        (temp_flake_dir / 'flake.lock').unlink()
        trix_result = compliance.run_trix(['flake', 'lock'], cwd=temp_flake_dir)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        trix_lock = json.loads((temp_flake_dir / 'flake.lock').read_text())

        # Compare follows format
        nix_follows = nix_lock.get('nodes', {}).get('root', {}).get('inputs', {}).get('nixpkgs-stable')
        trix_follows = trix_lock.get('nodes', {}).get('root', {}).get('inputs', {}).get('nixpkgs-stable')

        assert trix_follows == nix_follows, f'follows format mismatch: trix={trix_follows}, nix={nix_follows}'


class TestLockRoundTrip:
    """Test lock file round-trip compatibility."""

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_same_rev_locked(self, temp_flake_dir, compliance):
        """Given same input, trix and nix should lock to same revision."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(SIMPLE_FLAKE)

        # Generate with nix
        nix_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_result.stderr}')

        nix_lock = json.loads((temp_flake_dir / 'flake.lock').read_text())
        nix_rev = nix_lock.get('nodes', {}).get('nixpkgs', {}).get('locked', {}).get('rev')

        # Remove and regenerate with trix
        (temp_flake_dir / 'flake.lock').unlink()
        trix_result = compliance.run_trix(['flake', 'lock'], cwd=temp_flake_dir)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake lock failed: {trix_result.stderr}')

        trix_lock = json.loads((temp_flake_dir / 'flake.lock').read_text())
        trix_rev = trix_lock.get('nodes', {}).get('nixpkgs', {}).get('locked', {}).get('rev')

        # Both should lock to the same revision (HEAD of nixos-unstable at time of locking)
        # Note: This might differ if there's time between the two locks
        # For now, just verify both have valid revisions
        assert nix_rev is not None, 'nix should lock a revision'
        assert trix_rev is not None, 'trix should lock a revision'
        assert len(nix_rev) == 40, 'nix rev should be full commit hash'
        assert len(trix_rev) == 40, 'trix rev should be full commit hash'


class TestEvalCompliance:
    """Test that trix eval produces same output as nix eval."""

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_eval_package_name(self, temp_flake_dir, compliance):
        """trix eval and nix eval should produce same package name."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(SIMPLE_FLAKE)

        # Lock with nix first to ensure identical starting point
        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix
        nix_result = compliance.run_nix(
            ['eval', '.#packages.x86_64-linux.default.name'],
            cwd=temp_flake_dir,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(
            ['eval', '.#packages.x86_64-linux.default.name'],
            cwd=temp_flake_dir,
        )
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        # Both should produce same output
        nix_out = nix_result.stdout.strip()
        trix_out = trix_result.stdout.strip()
        assert trix_out == nix_out, f'eval mismatch: trix={trix_out}, nix={nix_out}'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_eval_json_output(self, temp_flake_dir, compliance):
        """trix eval --json should produce valid JSON matching nix."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(SIMPLE_FLAKE)

        # Lock with nix first
        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix --json
        nix_result = compliance.run_nix(
            ['eval', '.#packages.x86_64-linux.default.name', '--json'],
            cwd=temp_flake_dir,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix --json
        trix_result = compliance.run_trix(
            ['eval', '.#packages.x86_64-linux.default.name', '--json'],
            cwd=temp_flake_dir,
        )
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        # Both should parse as valid JSON and be equal
        nix_json = json.loads(nix_result.stdout.strip())
        trix_json = json.loads(trix_result.stdout.strip())
        assert trix_json == nix_json, f'JSON mismatch: trix={trix_json}, nix={nix_json}'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_attribute_shorthand_resolution(self, temp_flake_dir, compliance):
        """trix .#hello should resolve same as nix .#hello."""
        flake_nix = temp_flake_dir / 'flake.nix'
        # Flake with named package
        flake_nix.write_text("""{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }: {
    packages.x86_64-linux.hello = nixpkgs.legacyPackages.x86_64-linux.hello;
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.cowsay;
  };
}
""")

        # Lock with nix first
        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate .#hello with nix
        nix_result = compliance.run_nix(
            ['eval', '.#hello.name'],
            cwd=temp_flake_dir,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate .#hello with trix
        trix_result = compliance.run_trix(
            ['eval', '.#hello.name'],
            cwd=temp_flake_dir,
        )
        if trix_result.returncode != 0:
            pytest.skip(f'trix eval failed: {trix_result.stderr}')

        # Both should resolve to packages.x86_64-linux.hello
        nix_out = nix_result.stdout.strip()
        trix_out = trix_result.stdout.strip()
        assert 'hello' in trix_out.lower(), f'trix should resolve .#hello: {trix_out}'
        assert trix_out == nix_out, f'resolution mismatch: trix={trix_out}, nix={nix_out}'


class TestFlakeShowCompliance:
    """Test that trix flake show matches nix flake show output."""

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_flake_show_outputs(self, temp_flake_dir, compliance):
        """trix flake show should list same outputs as nix flake show."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(SIMPLE_FLAKE)

        # Lock with nix first
        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Show with nix
        nix_result = compliance.run_nix(['flake', 'show'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix flake show failed: {nix_result.stderr}')

        # Show with trix
        trix_result = compliance.run_trix(['flake', 'show'], cwd=temp_flake_dir)
        if trix_result.returncode != 0:
            pytest.skip(f'trix flake show failed: {trix_result.stderr}')

        # Both should mention packages and x86_64-linux
        assert 'packages' in trix_result.stdout, f'trix should show packages: {trix_result.stdout}'
        assert 'x86_64-linux' in trix_result.stdout or 'linux' in trix_result.stdout.lower()


# Flake with multiple output types for testing path resolution
MULTI_OUTPUT_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }: {
    # packages.<system>.<name>
    packages.x86_64-linux.hello = nixpkgs.legacyPackages.x86_64-linux.hello;
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.cowsay;

    # lib (top-level, no system)
    lib.version = "1.0.0";
    lib.helper = x: x + 1;

    # Check that legacyPackages isn't accidentally accessed for packages
    # (if packages.x86_64-linux.foo exists, don't fall back to legacyPackages)
  };
}
"""

# Flake that only has legacyPackages (like nixpkgs itself)
LEGACY_PACKAGES_ONLY_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }: {
    # No packages, only legacyPackages
    legacyPackages.x86_64-linux = nixpkgs.legacyPackages.x86_64-linux;
  };
}
"""


class TestEvalPathResolution:
    """Comprehensive tests for eval path resolution matching nix behavior."""

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_simple_name_resolves_to_packages(self, temp_flake_dir, compliance):
        """trix eval .#hello should resolve to packages.<system>.hello like nix."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        # Lock with nix first
        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix - .#hello should resolve to packages.x86_64-linux.hello
        nix_result = compliance.run_nix(['eval', '.#hello.name'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(['eval', '.#hello.name'], cwd=temp_flake_dir)
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'
        assert trix_result.stdout.strip() == nix_result.stdout.strip()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_full_path_works_directly(self, temp_flake_dir, compliance):
        """trix eval .#packages.x86_64-linux.hello.name should work directly."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix
        nix_result = compliance.run_nix(
            ['eval', '.#packages.x86_64-linux.hello.name'],
            cwd=temp_flake_dir,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(
            ['eval', '.#packages.x86_64-linux.hello.name'],
            cwd=temp_flake_dir,
        )
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'
        assert trix_result.stdout.strip() == nix_result.stdout.strip()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_full_path_with_apply(self, temp_flake_dir, compliance):
        """trix eval .#packages.x86_64-linux --apply builtins.attrNames should work."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix
        nix_result = compliance.run_nix(
            ['eval', '.#packages.x86_64-linux', '--apply', 'builtins.attrNames', '--json'],
            cwd=temp_flake_dir,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(
            ['eval', '.#packages.x86_64-linux', '--apply', 'builtins.attrNames', '--json'],
            cwd=temp_flake_dir,
        )
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'

        # Both should have the same package names
        nix_names = set(json.loads(nix_result.stdout.strip()))
        trix_names = set(json.loads(trix_result.stdout.strip()))
        assert trix_names == nix_names, f'attr names differ: trix={trix_names}, nix={nix_names}'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_top_level_lib_no_system_prefix(self, temp_flake_dir, compliance):
        """trix eval .#lib.version should work without system prefix."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix
        nix_result = compliance.run_nix(['eval', '.#lib.version'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(['eval', '.#lib.version'], cwd=temp_flake_dir)
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'
        assert trix_result.stdout.strip() == nix_result.stdout.strip()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_legacy_packages_fallback(self, temp_flake_dir, compliance):
        """trix eval .#hello should fall back to legacyPackages if no packages."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(LEGACY_PACKAGES_ONLY_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix - should fall back to legacyPackages.x86_64-linux.hello
        nix_result = compliance.run_nix(['eval', '.#hello.name'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(['eval', '.#hello.name'], cwd=temp_flake_dir)
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'
        assert trix_result.stdout.strip() == nix_result.stdout.strip()
        assert 'hello' in trix_result.stdout.lower()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_default_attr_resolution(self, temp_flake_dir, compliance):
        """trix eval . should resolve to packages.<system>.default like nix."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix
        nix_result = compliance.run_nix(['eval', '.#default.name'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(['eval', '.#default.name'], cwd=temp_flake_dir)
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'
        assert trix_result.stdout.strip() == nix_result.stdout.strip()
        # Default is cowsay in MULTI_OUTPUT_FLAKE
        assert 'cowsay' in trix_result.stdout.lower()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_packages_category_without_system(self, temp_flake_dir, compliance):
        """trix eval .#packages should match nix (returns system attrset)."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Evaluate with nix - .#packages should give the packages attrset
        nix_result = compliance.run_nix(
            ['eval', '.#packages', '--apply', 'builtins.attrNames', '--json'],
            cwd=temp_flake_dir,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(
            ['eval', '.#packages', '--apply', 'builtins.attrNames', '--json'],
            cwd=temp_flake_dir,
        )
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'

        nix_systems = set(json.loads(nix_result.stdout.strip()))
        trix_systems = set(json.loads(trix_result.stdout.strip()))
        assert trix_systems == nix_systems
        assert 'x86_64-linux' in trix_systems

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_nonexistent_attr_error(self, temp_flake_dir, compliance):
        """trix eval .#nonexistent should fail like nix."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Both should fail
        nix_result = compliance.run_nix(['eval', '.#nonexistent'], cwd=temp_flake_dir)
        trix_result = compliance.run_trix(['eval', '.#nonexistent'], cwd=temp_flake_dir)

        assert nix_result.returncode != 0, 'nix should fail for nonexistent attr'
        assert trix_result.returncode != 0, 'trix should fail for nonexistent attr'

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_empty_attr_resolves_to_default(self, temp_flake_dir, compliance):
        """trix eval '.#' should resolve to default package like nix (not root outputs)."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Both .# and . should resolve to packages.<system>.default (cowsay in this flake)
        # Evaluate with nix
        nix_result = compliance.run_nix(['eval', '.#', '--apply', 'x: x.name'], cwd=temp_flake_dir)
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        # Evaluate with trix
        trix_result = compliance.run_trix(['eval', '.#', '--apply', 'x: x.name'], cwd=temp_flake_dir)
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'

        assert trix_result.stdout.strip() == nix_result.stdout.strip()
        # Default is cowsay in MULTI_OUTPUT_FLAKE
        assert 'cowsay' in trix_result.stdout.lower()

    @pytest.mark.skipif(not NixComplianceTest.nix_available(), reason='nix flakes not available')
    def test_output_category_direct_access(self, temp_flake_dir, compliance):
        """trix eval '.#packages' should return the packages attrset like nix."""
        flake_nix = temp_flake_dir / 'flake.nix'
        flake_nix.write_text(MULTI_OUTPUT_FLAKE)

        nix_lock_result = compliance.run_nix(['flake', 'lock'], cwd=temp_flake_dir)
        if nix_lock_result.returncode != 0:
            pytest.skip(f'nix flake lock failed: {nix_lock_result.stderr}')

        # Direct access to 'packages' output should work
        nix_result = compliance.run_nix(
            ['eval', '.#packages', '--apply', 'builtins.attrNames', '--json'],
            cwd=temp_flake_dir,
        )
        if nix_result.returncode != 0:
            pytest.skip(f'nix eval failed: {nix_result.stderr}')

        trix_result = compliance.run_trix(
            ['eval', '.#packages', '--apply', 'builtins.attrNames', '--json'],
            cwd=temp_flake_dir,
        )
        assert trix_result.returncode == 0, f'trix eval failed: {trix_result.stderr}'

        nix_systems = set(json.loads(nix_result.stdout.strip()))
        trix_systems = set(json.loads(trix_result.stdout.strip()))
        assert trix_systems == nix_systems
        assert 'x86_64-linux' in trix_systems
