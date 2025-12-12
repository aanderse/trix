"""Tests for trix CLI commands."""

import json
import os
import shutil
import tempfile
from pathlib import Path
from unittest import mock

import pytest
from click.testing import CliRunner

from trix.cli import cli


@pytest.fixture
def runner():
    """Create a CLI test runner."""
    return CliRunner()


@pytest.fixture
def temp_dir():
    """Create a temporary directory for tests."""
    d = tempfile.mkdtemp()
    yield Path(d)
    shutil.rmtree(d)


@pytest.fixture
def sample_flake(temp_dir):
    """Create a sample flake.nix in a temp directory."""
    flake_nix = temp_dir / 'flake.nix'
    flake_nix.write_text("""{
  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }: {
    packages.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.hello;
  };
}
""")
    return temp_dir


@pytest.fixture
def sample_flake_with_lock(sample_flake):
    """Create a sample flake with an existing lock file."""
    lock_file = sample_flake / 'flake.lock'
    lock_file.write_text(
        json.dumps(
            {
                'nodes': {
                    'root': {'inputs': {'nixpkgs': 'nixpkgs'}},
                    'nixpkgs': {
                        'locked': {
                            'lastModified': 1700000000,
                            'narHash': 'sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
                            'owner': 'nixos',
                            'repo': 'nixpkgs',
                            'rev': 'abc123abc123abc123abc123abc123abc123abc1',
                            'type': 'github',
                        },
                        'original': {'owner': 'nixos', 'repo': 'nixpkgs', 'ref': 'nixos-unstable', 'type': 'github'},
                    },
                },
                'root': 'root',
                'version': 7,
            },
            indent=2,
        )
    )
    return sample_flake


class TestFlakeInit:
    """Tests for trix flake init command."""

    def test_init_help(self, runner):
        """Test that init --help works."""
        result = runner.invoke(cli, ['flake', 'init', '--help'])
        assert result.exit_code == 0
        assert 'Create a flake in the current directory' in result.output

    @mock.patch('trix.cli_flake.get_system')
    @mock.patch('subprocess.run')
    def test_init_copies_template_files(self, mock_run, mock_get_system, runner, temp_dir):
        """Test that init copies files from template."""
        mock_get_system.return_value = 'x86_64-linux'

        # Create a mock template in a "store path"
        store_path = temp_dir / 'store' / 'template-flake'
        store_path.mkdir(parents=True)
        (store_path / 'flake.nix').write_text('{ outputs = _: {}; }')

        template_dir = store_path / 'templates' / 'default'
        template_dir.mkdir(parents=True)
        (template_dir / 'flake.nix').write_text('{ description = "test"; }')
        # Make it read-only like the nix store
        (template_dir / 'flake.nix').chmod(0o444)

        # Mock nix flake prefetch
        mock_run.side_effect = [
            mock.Mock(returncode=0, stdout=json.dumps({'storePath': str(store_path)}), stderr=''),
            # Mock nix-instantiate for template info
            mock.Mock(
                returncode=0,
                stdout=json.dumps({'path': str(template_dir), 'description': 'A test template', 'welcomeText': ''}),
                stderr='',
            ),
        ]

        target_dir = temp_dir / 'project'
        target_dir.mkdir()

        with runner.isolated_filesystem(temp_dir=str(target_dir)):
            os.chdir(target_dir)
            result = runner.invoke(cli, ['flake', 'init'])

        assert result.exit_code == 0
        assert 'Wrote 1 file(s)' in result.output
        assert (target_dir / 'flake.nix').exists()
        # Check file is writable (not read-only like store)
        assert os.access(target_dir / 'flake.nix', os.W_OK)

    @mock.patch('trix.cli_flake.get_system')
    @mock.patch('subprocess.run')
    def test_init_does_not_overwrite_existing(self, mock_run, mock_get_system, runner, temp_dir):
        """Test that init does not overwrite existing files."""
        mock_get_system.return_value = 'x86_64-linux'

        store_path = temp_dir / 'store' / 'template-flake'
        store_path.mkdir(parents=True)
        (store_path / 'flake.nix').write_text('{ outputs = _: {}; }')

        template_dir = store_path / 'templates' / 'default'
        template_dir.mkdir(parents=True)
        (template_dir / 'flake.nix').write_text('{ description = "template"; }')

        mock_run.side_effect = [
            mock.Mock(returncode=0, stdout=json.dumps({'storePath': str(store_path)}), stderr=''),
            mock.Mock(
                returncode=0,
                stdout=json.dumps({'path': str(template_dir), 'description': '', 'welcomeText': ''}),
                stderr='',
            ),
        ]

        target_dir = temp_dir / 'project'
        target_dir.mkdir()
        existing_content = '# my existing flake'
        (target_dir / 'flake.nix').write_text(existing_content)

        with runner.isolated_filesystem(temp_dir=str(target_dir)):
            os.chdir(target_dir)
            result = runner.invoke(cli, ['flake', 'init'])

        assert result.exit_code == 0
        assert 'No files were written' in result.output
        # Verify original content preserved
        assert (target_dir / 'flake.nix').read_text() == existing_content

    @mock.patch('trix.cli_flake.get_system')
    @mock.patch('subprocess.run')
    def test_init_shows_welcome_text(self, mock_run, mock_get_system, runner, temp_dir):
        """Test that init displays welcome text if present."""
        mock_get_system.return_value = 'x86_64-linux'

        store_path = temp_dir / 'store' / 'template-flake'
        store_path.mkdir(parents=True)
        (store_path / 'flake.nix').write_text('{ outputs = _: {}; }')

        template_dir = store_path / 'templates' / 'default'
        template_dir.mkdir(parents=True)
        (template_dir / 'flake.nix').write_text('{}')

        welcome_text = 'Welcome to your new project!'

        mock_run.side_effect = [
            mock.Mock(returncode=0, stdout=json.dumps({'storePath': str(store_path)}), stderr=''),
            mock.Mock(
                returncode=0,
                stdout=json.dumps({'path': str(template_dir), 'description': '', 'welcomeText': welcome_text}),
                stderr='',
            ),
        ]

        target_dir = temp_dir / 'project'
        target_dir.mkdir()

        with runner.isolated_filesystem(temp_dir=str(target_dir)):
            os.chdir(target_dir)
            result = runner.invoke(cli, ['flake', 'init'])

        assert result.exit_code == 0
        assert welcome_text in result.output


class TestFlakeUpdate:
    """Tests for trix flake update command."""

    def test_update_help(self, runner):
        """Test that update --help works."""
        result = runner.invoke(cli, ['flake', 'update', '--help'])
        assert result.exit_code == 0
        assert '--override-input' in result.output
        assert 'Pin an input to a specific flake reference' in result.output

    def test_update_no_flake(self, runner, temp_dir):
        """Test update fails gracefully when no flake.nix exists."""
        with runner.isolated_filesystem(temp_dir=str(temp_dir)):
            os.chdir(temp_dir)
            result = runner.invoke(cli, ['flake', 'update'])
        assert result.exit_code == 1
        assert 'No flake.nix found' in result.output

    @mock.patch('trix.lock._prefetch_flake')
    def test_update_override_input(self, mock_prefetch, runner, sample_flake_with_lock):
        """Test update with --override-input."""
        new_rev = 'def456def456def456def456def456def456def4'
        mock_prefetch.return_value = {
            'hash': 'sha256-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=',
            'locked': {
                'lastModified': 1700000001,
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'rev': new_rev,
                'type': 'github',
            },
            'original': {'owner': 'NixOS', 'repo': 'nixpkgs', 'ref': 'nixos-24.05', 'type': 'github'},
        }

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['flake', 'update', '-o', 'nixpkgs', 'github:NixOS/nixpkgs/nixos-24.05'])

        assert result.exit_code == 0
        assert "Updated input 'nixpkgs'" in result.output

        # Verify lock file was updated
        lock_data = json.loads((sample_flake_with_lock / 'flake.lock').read_text())
        assert lock_data['nodes']['nixpkgs']['locked']['rev'] == new_rev

    @mock.patch('trix.lock._prefetch_flake')
    def test_update_override_input_shorthand(self, mock_prefetch, runner, sample_flake_with_lock):
        """Test update with branch/tag shorthand (e.g., nixos-24.05 instead of full ref)."""
        new_rev = 'def456def456def456def456def456def456def4'
        mock_prefetch.return_value = {
            'hash': 'sha256-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=',
            'locked': {
                'lastModified': 1700000001,
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'rev': new_rev,
                'type': 'github',
            },
            'original': {'owner': 'NixOS', 'repo': 'nixpkgs', 'ref': 'nixos-24.05', 'type': 'github'},
        }

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['flake', 'update', '-o', 'nixpkgs', 'nixos-24.05'])

        assert result.exit_code == 0
        # Verify the shorthand was expanded correctly
        mock_prefetch.assert_called_once()
        call_args = mock_prefetch.call_args[0]
        assert 'github:nixos/nixpkgs/nixos-24.05' in call_args[0]

    def test_update_override_nonexistent_input(self, runner, sample_flake_with_lock):
        """Test update fails for non-existent input."""
        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['flake', 'update', '-o', 'nonexistent', 'github:foo/bar'])

        assert result.exit_code == 1
        assert 'not found in flake.nix' in result.output


class TestBuild:
    """Tests for trix build command."""

    def test_build_help(self, runner):
        """Test that build --help works."""
        result = runner.invoke(cli, ['build', '--help'])
        assert result.exit_code == 0
        assert 'Build a package from flake.nix' in result.output
        assert '--out-link' in result.output
        assert '--no-link' in result.output

    @mock.patch('trix.cli.resolve_installable')
    def test_build_no_flake(self, mock_resolve, runner, temp_dir):
        """Test build fails gracefully when no flake.nix exists."""
        mock_resolve.side_effect = RuntimeError('No flake.nix found')

        with runner.isolated_filesystem(temp_dir=str(temp_dir)):
            os.chdir(temp_dir)
            result = runner.invoke(cli, ['build'])
        assert result.exit_code == 1

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_build_calls_nix_build(self, mock_ensure_lock, mock_nix_build, runner, sample_flake_with_lock):
        """Test build calls nix-build with correct arguments."""
        mock_ensure_lock.return_value = True
        mock_nix_build.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['build'])

        assert result.exit_code == 0
        mock_ensure_lock.assert_called_once()
        mock_nix_build.assert_called_once()
        call_kwargs = mock_nix_build.call_args[1]
        assert call_kwargs['out_link'] == 'result'
        assert call_kwargs['verbose'] is False

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_build_no_link(self, mock_ensure_lock, mock_nix_build, runner, sample_flake_with_lock):
        """Test build --no-link passes correct argument."""
        mock_ensure_lock.return_value = True
        mock_nix_build.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['build', '--no-link'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_build.call_args[1]
        assert call_kwargs['out_link'] is None

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_build_custom_out_link(self, mock_ensure_lock, mock_nix_build, runner, sample_flake_with_lock):
        """Test build -o custom-result uses custom symlink name."""
        mock_ensure_lock.return_value = True
        mock_nix_build.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['build', '-o', 'my-result'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_build.call_args[1]
        assert call_kwargs['out_link'] == 'my-result'

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_build_specific_package(self, mock_ensure_lock, mock_nix_build, runner, sample_flake_with_lock):
        """Test build .#hello builds specific package."""
        mock_ensure_lock.return_value = True
        mock_nix_build.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['build', '.#hello'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_build.call_args[1]
        assert 'hello' in call_kwargs['attr']

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_build_with_arg(self, mock_ensure_lock, mock_nix_build, runner, sample_flake_with_lock):
        """Test build --arg passes argument to nix-build."""
        mock_ensure_lock.return_value = True
        mock_nix_build.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['build', '--arg', 'foo', 'true'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_build.call_args[1]
        assert call_kwargs['extra_args'] == [('foo', 'true')]

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_build_with_argstr(self, mock_ensure_lock, mock_nix_build, runner, sample_flake_with_lock):
        """Test build --argstr passes argument to nix-build."""
        mock_ensure_lock.return_value = True
        mock_nix_build.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['build', '--argstr', 'version', '1.0.0'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_build.call_args[1]
        assert call_kwargs['extra_argstrs'] == [('version', '1.0.0')]

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_build_with_multiple_args(self, mock_ensure_lock, mock_nix_build, runner, sample_flake_with_lock):
        """Test build with multiple --arg and --argstr options."""
        mock_ensure_lock.return_value = True
        mock_nix_build.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(
                cli,
                [
                    'build',
                    '--arg',
                    'foo',
                    'true',
                    '--arg',
                    'bar',
                    '[ 1 2 ]',
                    '--argstr',
                    'version',
                    '1.0',
                    '--argstr',
                    'env',
                    'prod',
                ],
            )

        assert result.exit_code == 0
        call_kwargs = mock_nix_build.call_args[1]
        assert call_kwargs['extra_args'] == [('foo', 'true'), ('bar', '[ 1 2 ]')]
        assert call_kwargs['extra_argstrs'] == [('version', '1.0'), ('env', 'prod')]


class TestDevelop:
    """Tests for trix develop command."""

    def test_develop_help(self, runner):
        """Test that develop --help works."""
        result = runner.invoke(cli, ['develop', '--help'])
        assert result.exit_code == 0
        assert 'Enter a development shell' in result.output
        assert '--command' in result.output

    @mock.patch('trix.cli.resolve_installable')
    def test_develop_no_flake(self, mock_resolve, runner, temp_dir):
        """Test develop fails gracefully when no flake.nix exists."""
        mock_resolve.side_effect = RuntimeError('No flake.nix found')

        with runner.isolated_filesystem(temp_dir=str(temp_dir)):
            os.chdir(temp_dir)
            result = runner.invoke(cli, ['develop'])
        assert result.exit_code == 1

    @mock.patch('trix.cli.run_nix_shell')
    @mock.patch('trix.cli.ensure_lock')
    def test_develop_calls_nix_shell(self, mock_ensure_lock, mock_nix_shell, runner, sample_flake_with_lock):
        """Test develop calls nix-shell with correct arguments."""
        mock_ensure_lock.return_value = True

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['develop'])

        # Note: run_nix_shell calls execvp which replaces the process,
        # so in tests it will just return
        mock_ensure_lock.assert_called_once()
        mock_nix_shell.assert_called_once()
        call_kwargs = mock_nix_shell.call_args[1]
        assert call_kwargs['command'] is None

    @mock.patch('trix.cli.run_nix_shell')
    @mock.patch('trix.cli.ensure_lock')
    def test_develop_with_command(self, mock_ensure_lock, mock_nix_shell, runner, sample_flake_with_lock):
        """Test develop -c 'echo hi' passes command."""
        mock_ensure_lock.return_value = True

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['develop', '-c', 'echo hi'])

        mock_nix_shell.assert_called_once()
        call_kwargs = mock_nix_shell.call_args[1]
        assert call_kwargs['command'] == 'echo hi'

    @mock.patch('trix.cli.run_nix_shell')
    @mock.patch('trix.cli.ensure_lock')
    def test_develop_with_arg(self, mock_ensure_lock, mock_nix_shell, runner, sample_flake_with_lock):
        """Test develop --arg passes argument to nix-shell."""
        mock_ensure_lock.return_value = True

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['develop', '--arg', 'foo', 'true'])

        mock_nix_shell.assert_called_once()
        call_kwargs = mock_nix_shell.call_args[1]
        assert call_kwargs['extra_args'] == [('foo', 'true')]

    @mock.patch('trix.cli.run_nix_shell')
    @mock.patch('trix.cli.ensure_lock')
    def test_develop_with_argstr(self, mock_ensure_lock, mock_nix_shell, runner, sample_flake_with_lock):
        """Test develop --argstr passes argument to nix-shell."""
        mock_ensure_lock.return_value = True

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['develop', '--argstr', 'env', 'dev'])

        mock_nix_shell.assert_called_once()
        call_kwargs = mock_nix_shell.call_args[1]
        assert call_kwargs['extra_argstrs'] == [('env', 'dev')]


class TestEval:
    """Tests for trix eval command."""

    def test_eval_help(self, runner):
        """Test that eval --help works."""
        result = runner.invoke(cli, ['eval', '--help'])
        assert result.exit_code == 0
        assert 'Evaluate a flake attribute' in result.output
        assert '--json' in result.output
        assert '--raw' in result.output
        assert '--apply' in result.output

    @mock.patch('trix.cli.run_nix_eval')
    @mock.patch('trix.cli.ensure_lock')
    def test_eval_calls_run_nix_eval(self, mock_ensure_lock, mock_nix_eval, runner, sample_flake_with_lock):
        """Test eval calls run_nix_eval with correct arguments."""
        mock_ensure_lock.return_value = True
        mock_nix_eval.return_value = '"hello-1.0"'

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['eval', '.#packages.x86_64-linux.hello.name'])

        assert result.exit_code == 0
        assert '"hello-1.0"' in result.output
        mock_nix_eval.assert_called_once()
        call_kwargs = mock_nix_eval.call_args[1]
        assert 'packages.x86_64-linux.hello.name' in call_kwargs['attr']

    @mock.patch('trix.cli.run_nix_eval')
    @mock.patch('trix.cli.ensure_lock')
    def test_eval_json_flag(self, mock_ensure_lock, mock_nix_eval, runner, sample_flake_with_lock):
        """Test eval --json passes flag to run_nix_eval."""
        mock_ensure_lock.return_value = True
        mock_nix_eval.return_value = '{"name":"hello"}'

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['eval', '--json', '.#packages.x86_64-linux.hello.meta'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_eval.call_args[1]
        assert call_kwargs['output_json'] is True

    @mock.patch('trix.cli.run_nix_eval')
    @mock.patch('trix.cli.ensure_lock')
    def test_eval_raw_flag(self, mock_ensure_lock, mock_nix_eval, runner, sample_flake_with_lock):
        """Test eval --raw passes flag to run_nix_eval."""
        mock_ensure_lock.return_value = True
        mock_nix_eval.return_value = 'hello-1.0'

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['eval', '--raw', '.#packages.x86_64-linux.hello.name'])

        assert result.exit_code == 0
        assert 'hello-1.0' in result.output
        call_kwargs = mock_nix_eval.call_args[1]
        assert call_kwargs['raw'] is True

    @mock.patch('trix.cli.run_nix_eval')
    @mock.patch('trix.cli.ensure_lock')
    def test_eval_apply_flag(self, mock_ensure_lock, mock_nix_eval, runner, sample_flake_with_lock):
        """Test eval --apply passes function to run_nix_eval."""
        mock_ensure_lock.return_value = True
        mock_nix_eval.return_value = '["hello","world"]'

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['eval', '--apply', 'builtins.attrNames', '.#packages.x86_64-linux'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_eval.call_args[1]
        assert call_kwargs['apply_fn'] == 'builtins.attrNames'

    @mock.patch('trix.cli.run_nix_eval')
    @mock.patch('trix.cli.ensure_lock')
    def test_eval_with_arg(self, mock_ensure_lock, mock_nix_eval, runner, sample_flake_with_lock):
        """Test eval --arg passes argument to run_nix_eval."""
        mock_ensure_lock.return_value = True
        mock_nix_eval.return_value = '42'

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['eval', '--arg', 'foo', 'true', '.#test'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_eval.call_args[1]
        assert call_kwargs['extra_args'] == [('foo', 'true')]

    @mock.patch('trix.cli.run_nix_eval')
    @mock.patch('trix.cli.ensure_lock')
    def test_eval_with_argstr(self, mock_ensure_lock, mock_nix_eval, runner, sample_flake_with_lock):
        """Test eval --argstr passes argument to run_nix_eval."""
        mock_ensure_lock.return_value = True
        mock_nix_eval.return_value = '"1.0"'

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['eval', '--argstr', 'version', '1.0', '.#test'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_eval.call_args[1]
        assert call_kwargs['extra_argstrs'] == [('version', '1.0')]

    @mock.patch('trix.cli.run_nix_eval')
    @mock.patch('trix.cli.ensure_lock')
    def test_eval_default_attr(self, mock_ensure_lock, mock_nix_eval, runner, sample_flake_with_lock):
        """Test eval with just '.' passes 'default' attr - run_nix_eval handles fallback search."""
        mock_ensure_lock.return_value = True
        mock_nix_eval.return_value = '{"packages":{}}'

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['eval', '.'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_eval.call_args[1]
        # Raw attr is passed - run_nix_eval handles fallback search:
        # packages.<system>.default, legacyPackages.<system>.default, then default
        assert call_kwargs['attr'] == 'default'

    @mock.patch('trix.cli.run_nix_eval')
    @mock.patch('trix.cli.ensure_lock')
    def test_eval_root_outputs(self, mock_ensure_lock, mock_nix_eval, runner, sample_flake_with_lock):
        """Test eval with '.#' evaluates root outputs."""
        mock_ensure_lock.return_value = True
        mock_nix_eval.return_value = '{"packages":{}}'

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['eval', '.#'])

        assert result.exit_code == 0
        call_kwargs = mock_nix_eval.call_args[1]
        assert call_kwargs['attr'] == ''


class TestRun:
    """Tests for trix run command."""

    def test_run_help(self, runner):
        """Test that run --help works."""
        result = runner.invoke(cli, ['run', '--help'])
        assert result.exit_code == 0
        assert 'Build and run a package' in result.output

    @mock.patch('trix.cli.resolve_installable')
    def test_run_no_flake(self, mock_resolve, runner, temp_dir):
        """Test run fails gracefully when no flake.nix exists."""
        mock_resolve.side_effect = RuntimeError('No flake.nix found')

        with runner.isolated_filesystem(temp_dir=str(temp_dir)):
            os.chdir(temp_dir)
            result = runner.invoke(cli, ['run'])
        assert result.exit_code == 1

    @mock.patch('os.execv')
    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_run_executes_binary(
        self, mock_ensure_lock, mock_nix_build, mock_execv, runner, temp_dir, sample_flake_with_lock
    ):
        """Test run builds and executes the binary."""
        mock_ensure_lock.return_value = True

        # Create a fake store path with a binary
        store_path = temp_dir / 'nix' / 'store' / 'fake-hash-hello'
        bin_dir = store_path / 'bin'
        bin_dir.mkdir(parents=True)
        hello_bin = bin_dir / 'hello'
        hello_bin.write_text('#!/bin/sh\necho hello')
        hello_bin.chmod(0o755)

        mock_nix_build.return_value = str(store_path)

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['run'])

        mock_nix_build.assert_called_once()
        mock_execv.assert_called_once()
        exec_args = mock_execv.call_args[0]
        assert 'hello' in exec_args[0]

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_run_no_output(self, mock_ensure_lock, mock_nix_build, runner, sample_flake_with_lock):
        """Test run fails when build produces no output."""
        mock_ensure_lock.return_value = True
        mock_nix_build.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['run'])

        assert result.exit_code == 1
        assert 'Build produced no output' in result.output

    @mock.patch('trix.cli.run_nix_build')
    @mock.patch('trix.cli.ensure_lock')
    def test_run_no_bin_dir(self, mock_ensure_lock, mock_nix_build, runner, temp_dir, sample_flake_with_lock):
        """Test run fails when store path has no bin directory."""
        mock_ensure_lock.return_value = True

        # Create a fake store path without bin/
        store_path = temp_dir / 'nix' / 'store' / 'fake-hash-nobin'
        store_path.mkdir(parents=True)
        mock_nix_build.return_value = str(store_path)

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['run'])

        assert result.exit_code == 1
        assert 'No bin directory' in result.output


class TestFlakeMetadata:
    """Tests for trix flake metadata command."""

    def test_metadata_help(self, runner):
        """Test that metadata --help works."""
        result = runner.invoke(cli, ['flake', 'metadata', '--help'])
        assert result.exit_code == 0
        assert 'Show flake metadata and inputs' in result.output

    def test_metadata_no_flake(self, runner, temp_dir):
        """Test metadata fails gracefully when no flake.nix exists."""
        with runner.isolated_filesystem(temp_dir=str(temp_dir)):
            os.chdir(temp_dir)
            result = runner.invoke(cli, ['flake', 'metadata'])
        assert result.exit_code == 1
        assert 'No flake.nix found' in result.output

    @mock.patch('trix.cli_flake.ensure_lock')
    def test_metadata_shows_path(self, mock_ensure_lock, runner, sample_flake_with_lock):
        """Test metadata shows flake path."""
        mock_ensure_lock.return_value = True

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['flake', 'metadata'])

        assert result.exit_code == 0
        assert 'Path:' in result.output

    @mock.patch('trix.cli_flake.ensure_lock')
    def test_metadata_shows_inputs(self, mock_ensure_lock, runner, sample_flake_with_lock):
        """Test metadata shows locked inputs."""
        mock_ensure_lock.return_value = True

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['flake', 'metadata'])

        assert result.exit_code == 0
        assert 'Inputs:' in result.output
        assert 'nixpkgs' in result.output


class TestFlakeShow:
    """Tests for trix flake show command."""

    def test_show_help(self, runner):
        """Test that show --help works."""
        result = runner.invoke(cli, ['flake', 'show', '--help'])
        assert result.exit_code == 0
        assert 'Show flake outputs structure' in result.output

    def test_show_no_flake(self, runner, temp_dir):
        """Test show fails gracefully when no flake.nix exists."""
        with runner.isolated_filesystem(temp_dir=str(temp_dir)):
            os.chdir(temp_dir)
            result = runner.invoke(cli, ['flake', 'show'])
        assert result.exit_code == 1
        assert 'No flake.nix found' in result.output

    @mock.patch('trix.cli_flake.eval_flake_outputs')
    @mock.patch('trix.cli_flake.ensure_lock')
    def test_show_displays_outputs(self, mock_ensure_lock, mock_eval_outputs, runner, sample_flake_with_lock):
        """Test show displays flake outputs."""
        mock_ensure_lock.return_value = True
        mock_eval_outputs.return_value = {'packages': {'x86_64-linux': {'default': 'leaf', 'hello': 'leaf'}}}

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            result = runner.invoke(cli, ['flake', 'show'])

        assert result.exit_code == 0
        assert 'packages' in result.output

    @mock.patch('trix.cli_flake.eval_flake_outputs')
    @mock.patch('trix.cli_flake.ensure_lock')
    def test_show_no_outputs(self, mock_ensure_lock, mock_eval_outputs, runner, sample_flake_with_lock):
        """Test show handles empty outputs."""
        mock_ensure_lock.return_value = True
        mock_eval_outputs.return_value = None

        with runner.isolated_filesystem(temp_dir=str(sample_flake_with_lock)):
            os.chdir(sample_flake_with_lock)
            # Use -j 1 to use the sequential code path (eval_flake_outputs) instead of parallel
            result = runner.invoke(cli, ['flake', 'show', '-j', '1'])

        assert result.exit_code == 1
        assert 'No outputs found' in result.output


class TestFlakeLock:
    """Tests for trix flake lock command."""

    def test_lock_help(self, runner):
        """Test that lock --help works."""
        result = runner.invoke(cli, ['flake', 'lock', '--help'])
        assert result.exit_code == 0
        assert 'Create or update flake.lock' in result.output

    def test_lock_no_flake(self, runner, temp_dir):
        """Test lock fails gracefully when no flake.nix exists."""
        with runner.isolated_filesystem(temp_dir=str(temp_dir)):
            os.chdir(temp_dir)
            result = runner.invoke(cli, ['flake', 'lock'])
        assert result.exit_code == 1
        assert 'No flake.nix found' in result.output

    @mock.patch('trix.cli_flake.ensure_lock')
    def test_lock_creates_lockfile(self, mock_ensure_lock, runner, sample_flake):
        """Test lock creates a lock file."""
        mock_ensure_lock.return_value = True

        with runner.isolated_filesystem(temp_dir=str(sample_flake)):
            os.chdir(sample_flake)
            result = runner.invoke(cli, ['flake', 'lock'])

        assert result.exit_code == 0
        # Note: sync_inputs prints changes to stderr only when there are actual changes
        mock_ensure_lock.assert_called_once()


class TestProfile:
    """Tests for trix profile commands."""

    def test_profile_help(self, runner):
        """Test that profile --help works."""
        result = runner.invoke(cli, ['profile', '--help'])
        assert result.exit_code == 0
        assert 'Manage installed packages' in result.output

    def test_profile_list_help(self, runner):
        """Test that profile list --help works."""
        result = runner.invoke(cli, ['profile', 'list', '--help'])
        assert result.exit_code == 0
        assert 'List installed packages' in result.output

    def test_profile_add_help(self, runner):
        """Test that profile add --help works."""
        result = runner.invoke(cli, ['profile', 'add', '--help'])
        assert result.exit_code == 0
        assert 'Add packages' in result.output

    def test_profile_install_alias(self, runner):
        """Test that profile install works as alias for add."""
        result = runner.invoke(cli, ['profile', 'install', '--help'])
        assert result.exit_code == 0

    def test_profile_remove_help(self, runner):
        """Test that profile remove --help works."""
        result = runner.invoke(cli, ['profile', 'remove', '--help'])
        assert result.exit_code == 0
        assert 'Remove packages' in result.output

    @mock.patch('trix.cli_profile.prof.list_installed')
    def test_profile_list_empty(self, mock_list, runner):
        """Test profile list with no packages."""
        mock_list.return_value = []
        result = runner.invoke(cli, ['profile', 'list'])
        assert result.exit_code == 0
        assert 'No packages installed' in result.output

    @mock.patch('trix.cli_profile.prof.list_installed')
    def test_profile_list_packages(self, mock_list, runner):
        """Test profile list shows installed packages."""
        mock_list.return_value = [
            {
                'name': 'hello',
                'storePaths': ['/nix/store/abc-hello'],
                'originalUrl': 'path:/home/user/flake',
                'attrPath': 'packages.x86_64-linux.hello',
            },
            {
                'name': 'cowsay',
                'storePaths': ['/nix/store/xyz-cowsay'],
                'originalUrl': 'path:/home/user/flake2',
                'attrPath': 'packages.x86_64-linux.cowsay',
            },
        ]
        result = runner.invoke(cli, ['profile', 'list'])
        assert result.exit_code == 0
        assert 'hello' in result.output
        assert 'cowsay' in result.output

    @mock.patch('trix.cli_profile.prof.remove')
    def test_profile_remove(self, mock_remove, runner):
        """Test profile remove calls remove function."""
        mock_remove.return_value = True
        result = runner.invoke(cli, ['profile', 'remove', 'hello'])
        assert result.exit_code == 0
        assert 'Removed hello' in result.output
        mock_remove.assert_called_once_with('hello', verbose=False)

    @mock.patch('trix.cli_profile.prof.remove')
    def test_profile_remove_failure(self, mock_remove, runner):
        """Test profile remove handles failure."""
        mock_remove.return_value = False
        result = runner.invoke(cli, ['profile', 'remove', 'hello'])
        assert result.exit_code == 1


class TestMainCLI:
    """Tests for main CLI behavior."""

    def test_cli_help(self, runner):
        """Test that --help works."""
        result = runner.invoke(cli, ['--help'])
        assert result.exit_code == 0
        assert 'trix' in result.output
        assert 'build' in result.output
        assert 'develop' in result.output
        assert 'run' in result.output
        assert 'flake' in result.output
        assert 'profile' in result.output

    def test_cli_version(self, runner):
        """Test that --version flag is recognized."""
        # Note: --version may fail if package isn't installed, but it should
        # at least be recognized as a valid flag (not "No such option")
        result = runner.invoke(cli, ['--version'])
        # Either succeeds or fails with version lookup error, not "unknown option"
        assert 'No such option' not in result.output

    def test_cli_verbose_flag(self, runner):
        """Test that -v/--verbose flag is recognized."""
        result = runner.invoke(cli, ['-v', '--help'])
        assert result.exit_code == 0
        result = runner.invoke(cli, ['--verbose', '--help'])
        assert result.exit_code == 0
