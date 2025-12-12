"""Tests for flake parsing."""

from pathlib import Path
from unittest import mock

from trix.flake import (
    get_flake_description,
    get_flake_inputs,
    parse_flake_url,
    parse_installable,
    resolve_attr_path,
)


class TestParseFlakeUrl:
    """Tests for parse_flake_url function."""

    def test_parse_github_simple(self):
        """Test parsing simple github URL."""
        result = parse_flake_url('github:NixOS/nixpkgs')
        assert result == {'type': 'github', 'owner': 'NixOS', 'repo': 'nixpkgs'}

    def test_parse_github_with_ref(self):
        """Test parsing github URL with ref."""
        result = parse_flake_url('github:NixOS/nixpkgs/nixos-unstable')
        assert result == {
            'type': 'github',
            'owner': 'NixOS',
            'repo': 'nixpkgs',
            'ref': 'nixos-unstable',
        }

    def test_parse_github_with_query_ref(self):
        """Test parsing github URL with query string ref."""
        result = parse_flake_url('github:NixOS/nixpkgs?ref=nixos-24.05')
        assert result == {
            'type': 'github',
            'owner': 'NixOS',
            'repo': 'nixpkgs',
            'ref': 'nixos-24.05',
        }

    def test_parse_github_with_query_rev(self):
        """Test parsing github URL with query string rev."""
        result = parse_flake_url('github:NixOS/nixpkgs?rev=abc123def456')
        assert result == {
            'type': 'github',
            'owner': 'NixOS',
            'repo': 'nixpkgs',
            'rev': 'abc123def456',
        }

    def test_parse_github_with_ref_and_rev(self):
        """Test parsing github URL with both ref and rev query params."""
        result = parse_flake_url('github:NixOS/nixpkgs?ref=master&rev=abc123')
        assert result == {
            'type': 'github',
            'owner': 'NixOS',
            'repo': 'nixpkgs',
            'ref': 'master',
            'rev': 'abc123',
        }

    def test_parse_git_url(self):
        """Test parsing git+https URL."""
        result = parse_flake_url('git+https://example.com/repo.git')
        assert result == {'type': 'git', 'url': 'https://example.com/repo.git'}

    def test_parse_git_url_with_ref(self):
        """Test parsing git URL with ref."""
        result = parse_flake_url('git+https://example.com/repo.git?ref=main')
        assert result == {
            'type': 'git',
            'url': 'https://example.com/repo.git',
            'ref': 'main',
        }

    def test_parse_path_explicit(self):
        """Test parsing explicit path: URL."""
        result = parse_flake_url('path:./local')
        assert result == {'type': 'path', 'path': './local'}

    def test_parse_path_relative(self):
        """Test parsing relative path."""
        result = parse_flake_url('./local')
        assert result == {'type': 'path', 'path': './local'}

    def test_parse_path_absolute(self):
        """Test parsing absolute path."""
        result = parse_flake_url('/home/user/flake')
        assert result == {'type': 'path', 'path': '/home/user/flake'}

    def test_parse_unknown(self):
        """Test parsing unknown URL format."""
        result = parse_flake_url('something:weird')
        assert result == {'type': 'unknown', 'url': 'something:weird'}

    def test_parse_git_with_rev(self):
        """Test parsing git URL with rev query param."""
        result = parse_flake_url('git+https://example.com/repo.git?rev=abc123')
        assert result == {
            'type': 'git',
            'url': 'https://example.com/repo.git',
            'rev': 'abc123',
        }

    def test_parse_git_with_ref_and_rev(self):
        """Test parsing git URL with both ref and rev."""
        result = parse_flake_url('git+https://example.com/repo.git?ref=main&rev=abc123')
        assert result == {
            'type': 'git',
            'url': 'https://example.com/repo.git',
            'ref': 'main',
            'rev': 'abc123',
        }

    def test_parse_parent_path(self):
        """Test parsing parent relative path."""
        result = parse_flake_url('../sibling-flake')
        assert result == {'type': 'path', 'path': '../sibling-flake'}


class TestParseInstallable:
    """Tests for parse_installable function."""

    def test_parse_dot(self):
        """Test parsing '.' (current directory)."""
        with mock.patch('trix.flake.Path.cwd') as mock_cwd:
            mock_cwd.return_value = Path('/home/user/project')
            flake_dir, attr = parse_installable('.')
        assert flake_dir == Path('/home/user/project')
        assert attr == 'default'

    def test_parse_empty(self):
        """Test parsing empty string (current directory)."""
        with mock.patch('trix.flake.Path.cwd') as mock_cwd:
            mock_cwd.return_value = Path('/home/user/project')
            flake_dir, attr = parse_installable('')
        assert flake_dir == Path('/home/user/project')
        assert attr == 'default'

    def test_parse_with_attr(self):
        """Test parsing '.#hello'."""
        with mock.patch('trix.flake.Path.cwd') as mock_cwd:
            mock_cwd.return_value = Path('/home/user/project')
            flake_dir, attr = parse_installable('.#hello')
        assert flake_dir == Path('/home/user/project')
        assert attr == 'hello'

    def test_parse_with_nested_attr(self):
        """Test parsing '.#devShells.myshell'."""
        with mock.patch('trix.flake.Path.cwd') as mock_cwd:
            mock_cwd.return_value = Path('/home/user/project')
            flake_dir, attr = parse_installable('.#devShells.myshell')
        assert flake_dir == Path('/home/user/project')
        assert attr == 'devShells.myshell'

    def test_parse_path_with_attr(self, tmp_path):
        """Test parsing '/path/to/flake#pkg'."""
        flake_dir, attr = parse_installable(f'{tmp_path}#mypackage')
        assert flake_dir == tmp_path
        assert attr == 'mypackage'

    def test_parse_relative_path(self, tmp_path):
        """Test parsing relative path."""
        # Create a subdirectory
        subdir = tmp_path / 'myflake'
        subdir.mkdir()

        with mock.patch('trix.flake.Path.cwd') as mock_cwd:
            mock_cwd.return_value = tmp_path
            # Use resolve() which will use the real filesystem
            flake_dir, attr = parse_installable(str(subdir))
        assert flake_dir == subdir
        assert attr == 'default'


class TestResolveAttrPath:
    """Tests for resolve_attr_path function."""

    def test_simple_package(self):
        """Test resolving simple package name."""
        result = resolve_attr_path('hello', 'packages', 'x86_64-linux')
        assert result == 'packages.x86_64-linux.hello'

    def test_default_package(self):
        """Test resolving 'default'."""
        result = resolve_attr_path('default', 'packages', 'x86_64-linux')
        assert result == 'packages.x86_64-linux.default'

    def test_devshells_category(self):
        """Test resolving with devShells category."""
        result = resolve_attr_path('default', 'devShells', 'aarch64-darwin')
        assert result == 'devShells.aarch64-darwin.default'

    def test_category_without_system(self):
        """Test resolving 'packages.foo' adds system."""
        result = resolve_attr_path('packages.foo', 'packages', 'x86_64-linux')
        assert result == 'packages.x86_64-linux.foo'

    def test_devshells_without_system(self):
        """Test resolving 'devShells.myshell' adds system."""
        result = resolve_attr_path('devShells.myshell', 'packages', 'x86_64-linux')
        assert result == 'devShells.x86_64-linux.myshell'

    def test_apps_without_system(self):
        """Test resolving 'apps.myapp' adds system."""
        result = resolve_attr_path('apps.myapp', 'packages', 'aarch64-linux')
        assert result == 'apps.aarch64-linux.myapp'

    def test_checks_without_system(self):
        """Test resolving 'checks.mycheck' adds system."""
        result = resolve_attr_path('checks.mycheck', 'packages', 'x86_64-darwin')
        assert result == 'checks.x86_64-darwin.mycheck'

    def test_already_has_system(self):
        """Test that full path with system is unchanged."""
        result = resolve_attr_path('packages.x86_64-linux.hello', 'packages', 'x86_64-linux')
        assert result == 'packages.x86_64-linux.hello'

    def test_unknown_nested_path(self):
        """Test unknown nested path passes through unchanged for eval.nix fallback.

        Unknown dotted paths are passed through unchanged so that:
        1. Custom outputs like finixConfigurations.framework.config work
        2. eval.nix's getAttrPathWithFallback can handle the fallback logic
        """
        result = resolve_attr_path('something.other.entirely', 'packages', 'x86_64-linux')
        assert result == 'something.other.entirely'

    def test_custom_output_type_unchanged(self):
        """Test that custom output types like finixConfigurations pass through unchanged."""
        result = resolve_attr_path(
            'finixConfigurations.framework.config.system.topLevel',
            'packages',
            'x86_64-linux',
        )
        assert result == 'finixConfigurations.framework.config.system.topLevel'

    def test_nixos_configurations_unchanged(self):
        """Test that nixosConfigurations pass through unchanged."""
        result = resolve_attr_path(
            'nixosConfigurations.myhost.config.system.build.toplevel',
            'packages',
            'x86_64-linux',
        )
        # nixosConfigurations is in top_level_categories, so should be unchanged
        assert result == 'nixosConfigurations.myhost.config.system.build.toplevel'

    def test_lib_path_unchanged(self):
        """Test lib paths are unchanged (top-level output, no system)."""
        result = resolve_attr_path('lib.myFunc', 'packages', 'x86_64-linux')
        assert result == 'lib.myFunc'

    def test_overlays_path_unchanged(self):
        """Test overlays paths are unchanged (top-level output, no system)."""
        result = resolve_attr_path('overlays.default', 'packages', 'x86_64-linux')
        assert result == 'overlays.default'

    def test_different_systems(self):
        """Test with various system architectures."""
        systems = ['x86_64-linux', 'aarch64-linux', 'x86_64-darwin', 'aarch64-darwin']
        for system in systems:
            result = resolve_attr_path('hello', 'packages', system)
            assert result == f'packages.{system}.hello'


class TestGetFlakeInputs:
    """Tests for get_flake_inputs function."""

    @mock.patch('subprocess.run')
    def test_parses_github_input(self, mock_run):
        """Test parsing a simple github input."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[{"name":"nixpkgs","url":"github:NixOS/nixpkgs","follows":null,"flake":true,"nestedFollows":{}}]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert 'nixpkgs' in result
        assert result['nixpkgs']['type'] == 'github'
        assert result['nixpkgs']['owner'] == 'NixOS'
        assert result['nixpkgs']['repo'] == 'nixpkgs'

    @mock.patch('subprocess.run')
    def test_parses_github_with_ref(self, mock_run):
        """Test parsing github input with ref."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[{"name":"nixpkgs","url":"github:NixOS/nixpkgs/nixos-unstable","follows":null,"flake":true,"nestedFollows":{}}]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert result['nixpkgs']['ref'] == 'nixos-unstable'

    @mock.patch('subprocess.run')
    def test_parses_root_level_follows(self, mock_run):
        """Test parsing root-level follows."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[{"name":"nixpkgs","url":null,"follows":"ae/nixpkgs","flake":true,"nestedFollows":{}}]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert result['nixpkgs']['type'] == 'follows'
        assert result['nixpkgs']['follows'] == ['ae', 'nixpkgs']

    @mock.patch('subprocess.run')
    def test_parses_simple_follows(self, mock_run):
        """Test parsing simple follows (no slash)."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[{"name":"nixpkgs","url":null,"follows":"other","flake":true,"nestedFollows":{}}]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert result['nixpkgs']['type'] == 'follows'
        assert result['nixpkgs']['follows'] == ['other']

    @mock.patch('subprocess.run')
    def test_parses_nested_follows(self, mock_run):
        """Test parsing nested follows (inputs.ae.inputs.nixpkgs.follows)."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[{"name":"ae","url":"github:tvbeat/ae","follows":null,"flake":true,"nestedFollows":{"nixpkgs":"nixpkgs"}}]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert result['ae']['type'] == 'github'
        assert result['ae']['follows'] == {'nixpkgs': ['nixpkgs']}

    @mock.patch('subprocess.run')
    def test_parses_nested_follows_with_path(self, mock_run):
        """Test parsing nested follows with path."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[{"name":"ae","url":"github:tvbeat/ae","follows":null,"flake":true,"nestedFollows":{"nixpkgs":"other/nixpkgs"}}]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert result['ae']['follows'] == {'nixpkgs': ['other', 'nixpkgs']}

    @mock.patch('subprocess.run')
    def test_parses_non_flake_input(self, mock_run):
        """Test parsing flake=false input."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[{"name":"crate2nix","url":"github:kolloch/crate2nix","follows":null,"flake":false,"nestedFollows":{}}]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert result['crate2nix']['flake'] is False

    @mock.patch('subprocess.run')
    def test_parses_multiple_inputs(self, mock_run):
        """Test parsing multiple inputs."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[{"name":"nixpkgs","url":"github:NixOS/nixpkgs","follows":null,"flake":true,"nestedFollows":{}},{"name":"flake-utils","url":"github:numtide/flake-utils","follows":null,"flake":true,"nestedFollows":{}}]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert len(result) == 2
        assert 'nixpkgs' in result
        assert 'flake-utils' in result

    @mock.patch('subprocess.run')
    def test_returns_empty_on_failure(self, mock_run):
        """Test that failure returns empty dict."""
        mock_run.return_value = mock.Mock(
            returncode=1,
            stderr='error: syntax error',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert result == {}

    @mock.patch('subprocess.run')
    def test_returns_empty_for_no_inputs(self, mock_run):
        """Test that empty input list returns empty dict."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='[]',
        )

        result = get_flake_inputs(Path('/fake/flake'))

        assert result == {}


class TestGetFlakeDescription:
    """Tests for get_flake_description function."""

    @mock.patch('subprocess.run')
    def test_returns_description(self, mock_run):
        """Test returning a description."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='"My awesome flake"',
        )

        result = get_flake_description(Path('/fake/flake'))

        assert result == 'My awesome flake'

    @mock.patch('subprocess.run')
    def test_returns_none_for_null(self, mock_run):
        """Test returning None when description is null."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='null',
        )

        result = get_flake_description(Path('/fake/flake'))

        assert result is None

    @mock.patch('subprocess.run')
    def test_returns_none_on_failure(self, mock_run):
        """Test returning None on nix-instantiate failure."""
        mock_run.return_value = mock.Mock(
            returncode=1,
            stderr='error',
        )

        result = get_flake_description(Path('/fake/flake'))

        assert result is None

    @mock.patch('subprocess.run')
    def test_returns_none_for_empty_string(self, mock_run):
        """Test returning None for empty description."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='""',
        )

        result = get_flake_description(Path('/fake/flake'))

        # Empty string is falsy, so should return None
        assert result is None
