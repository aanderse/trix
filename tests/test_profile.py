"""Tests for profile module."""

import json
import os
from unittest import mock

from trix.profile import (
    collect_package_paths,
    create_profile_store_path,
    get_current_manifest,
    get_next_profile_number,
    get_profile_dir,
    is_local_path,
    list_installed,
    list_installed_names,
    remove,
    upgrade,
)


class TestGetProfileDir:
    """Tests for get_profile_dir function."""

    def test_returns_default_path_when_no_nix_profile(self):
        """Test default path when NIX_PROFILE not set."""
        with mock.patch.dict(os.environ, {}, clear=True):
            with mock.patch.dict(os.environ, {'USER': 'testuser', 'HOME': '/home/testuser'}):
                # Mock that the default path doesn't exist as a symlink
                with mock.patch('pathlib.Path.is_symlink', return_value=False):
                    result = get_profile_dir()
                    assert str(result) == '/nix/var/nix/profiles/per-user/testuser'

    def test_resolves_nix_profile_symlink(self, tmp_path):
        """Test resolving NIX_PROFILE symlink."""
        # Create a fake profile structure
        profile_dir = tmp_path / 'profiles'
        profile_dir.mkdir()
        profile_link = profile_dir / 'profile-1-link'
        profile_link.mkdir()

        main_profile = tmp_path / '.nix-profile'
        main_profile.symlink_to(profile_link)

        with mock.patch.dict(os.environ, {'NIX_PROFILE': str(main_profile)}):
            result = get_profile_dir()
            assert result == profile_dir


class TestGetCurrentManifest:
    """Tests for get_current_manifest function."""

    def test_returns_empty_manifest_when_no_profile(self):
        """Test returning empty manifest when profile doesn't exist."""
        with mock.patch('trix.profile.get_current_profile_path', return_value=None):
            result = get_current_manifest()
            assert result == {'version': 3, 'elements': {}}

    def test_returns_empty_manifest_when_no_manifest_file(self, tmp_path):
        """Test returning empty manifest when manifest.json doesn't exist."""
        with mock.patch('trix.profile.get_current_profile_path', return_value=tmp_path):
            result = get_current_manifest()
            assert result == {'version': 3, 'elements': {}}

    def test_reads_manifest_file(self, tmp_path):
        """Test reading existing manifest.json."""
        manifest = {
            'version': 3,
            'elements': {
                'hello': {
                    'active': True,
                    'priority': 5,
                    'storePaths': ['/nix/store/abc-hello'],
                    'originalUrl': 'path:/home/user/flake',
                    'attrPath': 'packages.x86_64-linux.hello',
                }
            },
        }
        (tmp_path / 'manifest.json').write_text(json.dumps(manifest))

        with mock.patch('trix.profile.get_current_profile_path', return_value=tmp_path):
            result = get_current_manifest()
            assert result == manifest


class TestGetNextProfileNumber:
    """Tests for get_next_profile_number function."""

    def test_returns_1_when_no_profiles(self, tmp_path):
        """Test returning 1 when no profile generations exist."""
        with mock.patch('trix.profile.get_profile_dir', return_value=tmp_path):
            result = get_next_profile_number()
            assert result == 1

    def test_returns_next_number(self, tmp_path):
        """Test returning next sequential number."""
        (tmp_path / 'profile-1-link').mkdir()
        (tmp_path / 'profile-2-link').mkdir()
        (tmp_path / 'profile-5-link').mkdir()

        with mock.patch('trix.profile.get_profile_dir', return_value=tmp_path):
            result = get_next_profile_number()
            assert result == 6

    def test_handles_non_profile_files(self, tmp_path):
        """Test ignoring non-profile files."""
        (tmp_path / 'profile-1-link').mkdir()
        (tmp_path / 'profile-invalid-link').mkdir()
        (tmp_path / 'other-file').touch()

        with mock.patch('trix.profile.get_profile_dir', return_value=tmp_path):
            result = get_next_profile_number()
            assert result == 2


class TestCollectPackagePaths:
    """Tests for collect_package_paths function."""

    def test_collects_single_package(self, tmp_path):
        """Test collecting paths from a single package."""
        pkg = tmp_path / 'pkg1'
        pkg.mkdir()
        (pkg / 'bin').mkdir()
        (pkg / 'share').mkdir()

        result = collect_package_paths([str(pkg)])

        assert set(result.keys()) == {'bin', 'share'}
        assert result['bin'] == [str(pkg / 'bin')]
        assert result['share'] == [str(pkg / 'share')]

    def test_collects_multiple_packages(self, tmp_path):
        """Test collecting paths from multiple packages."""
        pkg1 = tmp_path / 'pkg1'
        pkg1.mkdir()
        (pkg1 / 'bin').mkdir()

        pkg2 = tmp_path / 'pkg2'
        pkg2.mkdir()
        (pkg2 / 'bin').mkdir()
        (pkg2 / 'lib').mkdir()

        result = collect_package_paths([str(pkg1), str(pkg2)])

        assert set(result.keys()) == {'bin', 'lib'}
        assert len(result['bin']) == 2
        assert result['lib'] == [str(pkg2 / 'lib')]

    def test_skips_manifest_json(self, tmp_path):
        """Test that manifest.json is skipped."""
        pkg = tmp_path / 'pkg1'
        pkg.mkdir()
        (pkg / 'bin').mkdir()
        (pkg / 'manifest.json').touch()

        result = collect_package_paths([str(pkg)])

        assert 'manifest.json' not in result
        assert 'bin' in result


class TestListInstalled:
    """Tests for list_installed function."""

    def test_returns_empty_list_when_no_packages(self):
        """Test returning empty list when no packages installed."""
        with mock.patch('trix.profile.get_current_manifest', return_value={'version': 3, 'elements': {}}):
            result = list_installed()
            assert result == []

    def test_returns_package_info(self):
        """Test returning package information."""
        manifest = {
            'version': 3,
            'elements': {
                'hello': {
                    'active': True,
                    'priority': 5,
                    'storePaths': ['/nix/store/abc-hello'],
                    'originalUrl': 'path:/home/user/flake',
                    'attrPath': 'packages.x86_64-linux.hello',
                }
            },
        }

        with mock.patch('trix.profile.get_current_manifest', return_value=manifest):
            result = list_installed()

            assert len(result) == 1
            assert result[0]['name'] == 'hello'
            assert result[0]['storePaths'] == ['/nix/store/abc-hello']
            assert result[0]['originalUrl'] == 'path:/home/user/flake'

    def test_filters_inactive_packages(self):
        """Test that inactive packages are filtered."""
        manifest = {
            'version': 3,
            'elements': {
                'hello': {'active': True, 'storePaths': ['/nix/store/abc-hello']},
                'removed': {'active': False, 'storePaths': ['/nix/store/xyz-removed']},
            },
        }

        with mock.patch('trix.profile.get_current_manifest', return_value=manifest):
            result = list_installed()

            assert len(result) == 1
            assert result[0]['name'] == 'hello'


class TestListInstalledNames:
    """Tests for list_installed_names function."""

    def test_returns_names_only(self):
        """Test returning only package names."""
        with mock.patch(
            'trix.profile.list_installed',
            return_value=[
                {'name': 'hello', 'storePaths': []},
                {'name': 'world', 'storePaths': []},
            ],
        ):
            result = list_installed_names()
            assert result == ['hello', 'world']


class TestIsLocalPath:
    """Tests for is_local_path function."""

    def test_dot_is_local(self):
        """Test that '.' is considered local."""
        assert is_local_path('.') is True

    def test_empty_is_local(self):
        """Test that empty string is considered local."""
        assert is_local_path('') is True

    def test_relative_paths_are_local(self):
        """Test that relative paths are local."""
        assert is_local_path('./foo') is True
        assert is_local_path('../foo') is True

    def test_absolute_paths_are_local(self):
        """Test that absolute paths are local."""
        assert is_local_path('/home/user/flake') is True

    def test_tilde_paths_are_local(self):
        """Test that tilde paths are local."""
        assert is_local_path('~/flake') is True

    def test_remote_refs_are_not_local(self):
        """Test that remote references are not local."""
        assert is_local_path('github:NixOS/nixpkgs') is False
        assert is_local_path('nixpkgs') is False


class TestRemove:
    """Tests for remove function."""

    def test_removes_existing_package(self, tmp_path):
        """Test removing an existing package."""
        manifest = {
            'version': 3,
            'elements': {
                'hello': {
                    'active': True,
                    'storePaths': ['/nix/store/abc-hello'],
                }
            },
        }

        with mock.patch('trix.profile.get_current_manifest', return_value=manifest):
            with mock.patch('trix.profile.create_profile_store_path', return_value='/nix/store/new-profile'):
                with mock.patch('trix.profile.switch_profile'):
                    result = remove('hello')
                    assert result is True

    def test_fails_for_nonexistent_package(self):
        """Test that removing nonexistent package fails."""
        with mock.patch('trix.profile.get_current_manifest', return_value={'version': 3, 'elements': {}}):
            result = remove('nonexistent')
            assert result is False

    def test_partial_match(self):
        """Test partial name matching."""
        manifest = {
            'version': 3,
            'elements': {
                'hello-2.12.2': {
                    'active': True,
                    'storePaths': ['/nix/store/abc-hello'],
                }
            },
        }

        with mock.patch('trix.profile.get_current_manifest', return_value=manifest):
            with mock.patch('trix.profile.create_profile_store_path', return_value='/nix/store/new-profile'):
                with mock.patch('trix.profile.switch_profile'):
                    result = remove('hello')
                    assert result is True

    def test_ambiguous_match_fails(self):
        """Test that ambiguous partial matches fail."""
        manifest = {
            'version': 3,
            'elements': {
                'hello-2.12': {'active': True, 'storePaths': []},
                'hello-world': {'active': True, 'storePaths': []},
            },
        }

        with mock.patch('trix.profile.get_current_manifest', return_value=manifest):
            result = remove('hello')
            assert result is False


class TestUpgrade:
    """Tests for upgrade function."""

    def test_no_packages_returns_zero(self):
        """Test upgrading with no packages returns zero counts."""
        with mock.patch('trix.profile.get_current_manifest', return_value={'version': 3, 'elements': {}}):
            upgraded, skipped = upgrade()
            assert upgraded == 0
            assert skipped == 0

    def test_no_local_packages_returns_zero(self):
        """Test upgrading with no local packages returns zero counts."""
        manifest = {
            'version': 3,
            'elements': {
                'hello': {
                    'active': True,
                    'originalUrl': 'github:NixOS/nixpkgs',
                    'storePaths': ['/nix/store/abc-hello'],
                }
            },
        }

        with mock.patch('trix.profile.get_current_manifest', return_value=manifest):
            upgraded, skipped = upgrade()
            assert upgraded == 0
            assert skipped == 0

    def test_skips_missing_flake_paths(self, tmp_path):
        """Test that missing flake paths are skipped."""
        manifest = {
            'version': 3,
            'elements': {
                'hello': {
                    'active': True,
                    'originalUrl': 'path:/nonexistent/path',
                    'attrPath': 'packages.x86_64-linux.hello',
                    'storePaths': ['/nix/store/abc-hello'],
                }
            },
        }

        with mock.patch('trix.profile.get_current_manifest', return_value=manifest):
            upgraded, skipped = upgrade()
            assert upgraded == 0
            assert skipped == 1

    def test_upgrade_filters_by_name(self):
        """Test upgrading specific package by name."""
        manifest = {
            'version': 3,
            'elements': {
                'hello': {
                    'active': True,
                    'originalUrl': 'path:/nonexistent/hello',
                    'attrPath': 'packages.x86_64-linux.hello',
                    'storePaths': ['/nix/store/abc-hello'],
                },
                'world': {
                    'active': True,
                    'originalUrl': 'path:/nonexistent/world',
                    'attrPath': 'packages.x86_64-linux.world',
                    'storePaths': ['/nix/store/abc-world'],
                },
            },
        }

        with mock.patch('trix.profile.get_current_manifest', return_value=manifest):
            # Only 'hello' should be attempted (and skipped due to missing path)
            upgraded, skipped = upgrade('hello')
            assert upgraded == 0
            assert skipped == 1  # Only hello attempted


class TestCreateProfileStorePath:
    """Tests for create_profile_store_path function."""

    def test_creates_manifest(self, tmp_path):
        """Test that manifest.json is created in profile."""
        manifest = {'version': 3, 'elements': {'hello': {'storePaths': []}}}

        # Mock nix-store --add to return a fake path
        with mock.patch('subprocess.run') as mock_run:
            mock_run.return_value = mock.Mock(
                returncode=0,
                stdout='/nix/store/fake-profile\n',
            )

            result = create_profile_store_path(manifest, [])

            assert result == '/nix/store/fake-profile'
            mock_run.assert_called_once()

    def test_creates_symlinks_for_packages(self, tmp_path):
        """Test that package contents are symlinked."""
        pkg = tmp_path / 'pkg'
        pkg.mkdir()
        (pkg / 'bin').mkdir()
        (pkg / 'bin' / 'hello').touch()

        manifest = {'version': 3, 'elements': {}}

        with mock.patch('subprocess.run') as mock_run:
            mock_run.return_value = mock.Mock(
                returncode=0,
                stdout='/nix/store/fake-profile\n',
            )

            create_profile_store_path(manifest, [str(pkg)])

            # Verify nix-store --add was called
            call_args = mock_run.call_args[0][0]
            assert call_args[0] == 'nix-store'
            assert call_args[1] == '--add'
