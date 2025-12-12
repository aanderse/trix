"""Shared fixtures and utilities for trix tests."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

import pytest


@dataclass
class ComparisonResult:
    """Result of comparing trix vs nix output."""

    trix_output: str
    nix_output: str
    matches: bool
    diff: str | None = None


class NixComplianceTest:
    """Utilities for compliance tests comparing trix vs nix."""

    @staticmethod
    def nix_available() -> bool:
        """Check if nix flakes is available."""
        try:
            result = subprocess.run(
                [
                    'nix',
                    '--extra-experimental-features',
                    'nix-command flakes',
                    'flake',
                    '--help',
                ],
                capture_output=True,
                timeout=10,
            )
            return result.returncode == 0
        except (subprocess.TimeoutExpired, FileNotFoundError):
            return False

    @staticmethod
    def run_nix(cmd: list[str], cwd: Path, timeout: int = 60) -> subprocess.CompletedProcess:
        """Run a nix command with flakes enabled."""
        full_cmd = ['nix', '--extra-experimental-features', 'nix-command flakes'] + cmd
        return subprocess.run(full_cmd, capture_output=True, text=True, cwd=cwd, timeout=timeout)

    @staticmethod
    def run_trix(cmd: list[str], cwd: Path, timeout: int = 60) -> subprocess.CompletedProcess:
        """Run a trix command."""
        # Use sys.executable to ensure we use the same Python with dependencies installed
        full_cmd = [sys.executable, '-m', 'trix.cli'] + cmd
        env = os.environ.copy()
        # Ensure trix module is importable
        src_path = Path(__file__).parent.parent / 'src'
        env['PYTHONPATH'] = str(src_path) + ':' + env.get('PYTHONPATH', '')
        return subprocess.run(full_cmd, capture_output=True, text=True, cwd=cwd, env=env, timeout=timeout)

    @staticmethod
    def compare_locks(trix_lock: dict, nix_lock: dict, ignore_timestamps: bool = True) -> ComparisonResult:
        """Compare lock files for semantic equality."""

        def normalize(lock: dict) -> dict:
            """Normalize lock for comparison."""
            normalized = json.loads(json.dumps(lock, sort_keys=True))
            if ignore_timestamps:
                for node in normalized.get('nodes', {}).values():
                    if isinstance(node, dict) and 'locked' in node:
                        node['locked'].pop('lastModified', None)
            return normalized

        t_norm = normalize(trix_lock)
        n_norm = normalize(nix_lock)
        matches = t_norm == n_norm

        diff = None
        if not matches:
            import difflib

            diff = '\n'.join(
                difflib.unified_diff(
                    json.dumps(t_norm, indent=2, sort_keys=True).splitlines(),
                    json.dumps(n_norm, indent=2, sort_keys=True).splitlines(),
                    fromfile='trix',
                    tofile='nix',
                )
            )

        return ComparisonResult(
            trix_output=json.dumps(trix_lock, indent=2),
            nix_output=json.dumps(nix_lock, indent=2),
            matches=matches,
            diff=diff,
        )


@pytest.fixture
def compliance():
    """Provide compliance testing utilities."""
    return NixComplianceTest()


@pytest.fixture
def nix_available():
    """Skip test if nix flakes not available."""
    if not NixComplianceTest.nix_available():
        pytest.skip('nix flakes not available')


@pytest.fixture
def temp_flake_dir():
    """Create a temporary directory for flake tests."""
    d = tempfile.mkdtemp(prefix='trix_test_')
    yield Path(d)
    shutil.rmtree(d, ignore_errors=True)


# Sample flake configurations for testing
SIMPLE_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }: {
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""

FOLLOWS_SIMPLE_FLAKE = """{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    flake-utils.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils }: {
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""

FOLLOWS_ROOT_FLAKE = """{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixpkgs-stable.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, nixpkgs-stable }: {
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
"""

# A flake that doesn't use self as source - should never copy current dir
NO_SELF_FLAKE = """{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let pkgs = nixpkgs.legacyPackages.x86_64-linux;
    in {
      packages.x86_64-linux.default = pkgs.hello;
    };
}
"""
