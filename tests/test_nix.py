"""Tests for nix module."""

import os
from pathlib import Path
from unittest import mock

import pytest

from trix.nix import _get_clean_env, eval_expr, get_nix_dir, get_system, run_nix_build, run_nix_eval


class TestGetCleanEnv:
    """Tests for _get_clean_env function."""

    def test_returns_copy_of_environ(self):
        """Test that it returns a copy, not the original."""
        env = _get_clean_env()
        assert env is not os.environ

    def test_preserves_normal_vars(self):
        """Test that normal environment variables are preserved."""
        with mock.patch.dict(os.environ, {'MY_VAR': 'my_value', 'PATH': '/usr/bin'}):
            env = _get_clean_env()
        assert env.get('MY_VAR') == 'my_value'
        assert env.get('PATH') == '/usr/bin'

    def test_removes_stale_tmpdir(self, tmp_path):
        """Test that stale TMPDIR is removed."""
        nonexistent = '/tmp/nonexistent-12345-abcdef'
        with mock.patch.dict(os.environ, {'TMPDIR': nonexistent}, clear=False):
            env = _get_clean_env()
        assert 'TMPDIR' not in env

    def test_removes_valid_tmpdir(self, tmp_path):
        """Test that TMPDIR is always removed to let nix create its own."""
        with mock.patch.dict(os.environ, {'TMPDIR': str(tmp_path)}, clear=False):
            env = _get_clean_env()
        assert 'TMPDIR' not in env

    def test_handles_missing_tmpdir(self):
        """Test that missing TMPDIR is handled gracefully."""
        with mock.patch.dict(os.environ, {}, clear=True):
            # Set minimal required env vars
            os.environ['PATH'] = '/usr/bin'
            env = _get_clean_env()
        assert 'TMPDIR' not in env or env.get('TMPDIR') is None


class TestGetNixDir:
    """Tests for get_nix_dir function."""

    def test_finds_dev_nix_dir(self):
        """Test that it finds the dev nix/ directory."""
        # In the test environment, we should find the dev nix dir
        nix_dir = get_nix_dir()
        assert nix_dir.is_dir()
        assert (nix_dir / 'eval.nix').exists()

    def test_nix_dir_contains_required_files(self):
        """Test that nix dir contains expected files."""
        nix_dir = get_nix_dir()
        assert (nix_dir / 'eval.nix').exists()
        assert (nix_dir / 'inputs.nix').exists()

    def test_raises_when_not_found(self, tmp_path, monkeypatch):
        """Test that RuntimeError is raised when nix dir not found."""
        # Patch __file__ to point to a location with no nix dir
        fake_trix = tmp_path / 'src' / 'trix'
        fake_trix.mkdir(parents=True)

        import trix.nix as nix_module

        original_file = nix_module.__file__

        try:
            monkeypatch.setattr(nix_module, '__file__', str(fake_trix / 'nix.py'))
            with pytest.raises(RuntimeError, match='Cannot find nix/ directory'):
                get_nix_dir()
        finally:
            monkeypatch.setattr(nix_module, '__file__', original_file)


class TestGetSystem:
    """Tests for get_system function."""

    def test_returns_string(self):
        """Test that get_system returns a string."""
        # Clear cache first
        get_system.cache_clear()

        system = get_system()
        assert isinstance(system, str)
        assert '-' in system  # e.g., x86_64-linux

    def test_caches_result(self):
        """Test that result is cached using lru_cache."""
        # Clear the cache first
        get_system.cache_clear()

        system1 = get_system()
        system2 = get_system()
        assert system1 == system2
        # Check cache info shows 1 miss (first call) and 1 hit (second call)
        cache_info = get_system.cache_info()
        assert cache_info.hits >= 1
        assert cache_info.misses >= 1

    def test_uses_cached_value(self):
        """Test that cached value is used without subprocess call."""
        # Clear and prime the cache
        get_system.cache_clear()
        first_result = get_system()

        # Second call should use cache, not subprocess
        with mock.patch('subprocess.run') as mock_run:
            system = get_system()

        assert system == first_result
        mock_run.assert_not_called()

    @mock.patch('subprocess.run')
    def test_fallback_on_nix_failure(self, mock_run):
        """Test fallback when nix-instantiate fails."""
        get_system.cache_clear()

        mock_run.return_value = mock.Mock(returncode=1, stderr='error')

        with mock.patch('platform.machine', return_value='x86_64'):
            with mock.patch('platform.system', return_value='Linux'):
                system = get_system()

        assert system == 'x86_64-linux'

        # Reset cache for other tests
        get_system.cache_clear()

    @mock.patch('subprocess.run')
    def test_parses_nix_output(self, mock_run):
        """Test parsing of nix-instantiate output."""
        get_system.cache_clear()

        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='"aarch64-darwin"',
        )

        system = get_system()
        assert system == 'aarch64-darwin'

        # Reset cache for other tests
        get_system.cache_clear()


class TestEvalExpr:
    """Tests for eval_expr function."""

    @mock.patch('subprocess.run')
    def test_evaluates_simple_expr(self, mock_run):
        """Test evaluating a simple expression."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='42',
        )

        result = eval_expr('1 + 41')
        assert result == 42

    @mock.patch('subprocess.run')
    def test_evaluates_string_expr(self, mock_run):
        """Test evaluating a string expression."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='"hello"',
        )

        result = eval_expr('"hello"')
        assert result == 'hello'

    @mock.patch('subprocess.run')
    def test_evaluates_attrset_expr(self, mock_run):
        """Test evaluating an attrset expression."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='{"a":1,"b":2}',
        )

        result = eval_expr('{ a = 1; b = 2; }')
        assert result == {'a': 1, 'b': 2}

    @mock.patch('subprocess.run')
    def test_raises_on_failure(self, mock_run):
        """Test that RuntimeError is raised on failure."""
        mock_run.return_value = mock.Mock(
            returncode=1,
            stderr='error: syntax error',
        )

        with pytest.raises(RuntimeError, match='nix-instantiate failed'):
            eval_expr('invalid nix')

    @mock.patch('subprocess.run')
    def test_passes_cwd(self, mock_run):
        """Test that cwd parameter is passed to subprocess."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='1',
        )

        eval_expr('1', cwd=Path('/tmp'))

        # Check that cwd was passed
        call_kwargs = mock_run.call_args.kwargs
        assert call_kwargs.get('cwd') == Path('/tmp')

    @mock.patch('subprocess.run')
    def test_uses_clean_env(self, mock_run):
        """Test that clean environment is used."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='1',
        )

        eval_expr('1')

        # Check that env was passed (not None)
        call_kwargs = mock_run.call_args.kwargs
        assert 'env' in call_kwargs
        assert call_kwargs['env'] is not None


class TestRunNixBuild:
    """Tests for run_nix_build function."""

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_passes_extra_args(self, mock_run, mock_nix_dir, mock_system):
        """Test that extra_args are passed to nix-build."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(returncode=0)

        run_nix_build(
            flake_dir=Path('/fake/flake'),
            attr='packages.x86_64-linux.hello',
            out_link=None,
            extra_args=[('foo', 'true'), ('bar', '[ 1 2 3 ]')],
        )

        call_args = mock_run.call_args[0][0]
        # Check that foo and bar args are in the command
        cmd_str = ' '.join(call_args)
        assert '--arg foo true' in cmd_str
        assert '--arg bar [ 1 2 3 ]' in cmd_str

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_passes_extra_argstrs(self, mock_run, mock_nix_dir, mock_system):
        """Test that extra_argstrs are passed to nix-build."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(returncode=0)

        run_nix_build(
            flake_dir=Path('/fake/flake'),
            attr='packages.x86_64-linux.hello',
            out_link=None,
            extra_argstrs=[('version', '1.0.0'), ('env', 'production')],
        )

        call_args = mock_run.call_args[0][0]
        # Find the extra --argstr options (after the standard ones)
        # Standard argstrs are: system, attr
        argstr_indices = [i for i, x in enumerate(call_args) if x == '--argstr']

        # Should have at least 4 --argstr (system, attr, version, env)
        assert len(argstr_indices) >= 4

        # Check that version and env are in there
        cmd_str = ' '.join(call_args)
        assert '--argstr version 1.0.0' in cmd_str
        assert '--argstr env production' in cmd_str

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_no_extra_args_by_default(self, mock_run, mock_nix_dir, mock_system):
        """Test that no extra args are added by default."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(returncode=0)

        run_nix_build(
            flake_dir=Path('/fake/flake'),
            attr='packages.x86_64-linux.hello',
            out_link=None,
        )

        call_args = mock_run.call_args[0][0]
        # Count --arg occurrences (should only have the standard flakeDir)
        arg_count = call_args.count('--arg')
        assert arg_count == 1  # Only flakeDir

        # Count --argstr occurrences (should only have system and attr)
        argstr_count = call_args.count('--argstr')
        assert argstr_count == 2  # system and attr


class TestRunNixEval:
    """Tests for run_nix_eval function."""

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_evaluates_flake_attr(self, mock_run, mock_nix_dir, mock_system, tmp_path):
        """Test evaluating a flake attribute."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='"hello-1.0"',
            stderr='',
        )

        # Create a fake flake dir with lock file
        flake_dir = tmp_path / 'flake'
        flake_dir.mkdir()
        (flake_dir / 'flake.lock').write_text('{"nodes":{},"root":"root","version":7}')

        result = run_nix_eval(
            flake_dir=flake_dir,
            attr='packages.x86_64-linux.hello.name',
        )

        assert result == '"hello-1.0"'
        mock_run.assert_called_once()

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_json_output(self, mock_run, mock_nix_dir, mock_system, tmp_path):
        """Test --json flag adds --json to command."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='{"name":"hello"}',
            stderr='',
        )

        flake_dir = tmp_path / 'flake'
        flake_dir.mkdir()
        (flake_dir / 'flake.lock').write_text('{"nodes":{},"root":"root","version":7}')

        result = run_nix_eval(
            flake_dir=flake_dir,
            attr='packages.x86_64-linux.hello.meta',
            output_json=True,
        )

        assert result == '{"name":"hello"}'
        call_args = mock_run.call_args[0][0]
        assert '--json' in call_args

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_raw_output_strips_quotes(self, mock_run, mock_nix_dir, mock_system, tmp_path):
        """Test --raw strips quotes from string output."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='"hello-1.0"',
            stderr='',
        )

        flake_dir = tmp_path / 'flake'
        flake_dir.mkdir()
        (flake_dir / 'flake.lock').write_text('{"nodes":{},"root":"root","version":7}')

        result = run_nix_eval(
            flake_dir=flake_dir,
            attr='packages.x86_64-linux.hello.name',
            raw=True,
        )

        assert result == 'hello-1.0'

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_apply_function(self, mock_run, mock_nix_dir, mock_system, tmp_path):
        """Test --apply wraps result in function call."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='["foo","bar"]',
            stderr='',
        )

        flake_dir = tmp_path / 'flake'
        flake_dir.mkdir()
        (flake_dir / 'flake.lock').write_text('{"nodes":{},"root":"root","version":7}')

        result = run_nix_eval(
            flake_dir=flake_dir,
            attr='packages.x86_64-linux',
            apply_fn='builtins.attrNames',
        )

        assert result == '["foo","bar"]'
        # Check that the apply function is in the expression
        call_args = mock_run.call_args[0][0]
        expr_idx = call_args.index('--expr') + 1
        expr = call_args[expr_idx]
        assert 'builtins.attrNames' in expr

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_passes_extra_args(self, mock_run, mock_nix_dir, mock_system, tmp_path):
        """Test that extra_args are passed to nix-instantiate."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='42',
            stderr='',
        )

        flake_dir = tmp_path / 'flake'
        flake_dir.mkdir()
        (flake_dir / 'flake.lock').write_text('{"nodes":{},"root":"root","version":7}')

        run_nix_eval(
            flake_dir=flake_dir,
            attr='packages.x86_64-linux.hello',
            extra_args=[('foo', 'true')],
        )

        call_args = mock_run.call_args[0][0]
        cmd_str = ' '.join(call_args)
        assert '--arg foo true' in cmd_str

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_passes_extra_argstrs(self, mock_run, mock_nix_dir, mock_system, tmp_path):
        """Test that extra_argstrs are passed to nix-instantiate."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='42',
            stderr='',
        )

        flake_dir = tmp_path / 'flake'
        flake_dir.mkdir()
        (flake_dir / 'flake.lock').write_text('{"nodes":{},"root":"root","version":7}')

        run_nix_eval(
            flake_dir=flake_dir,
            attr='packages.x86_64-linux.hello',
            extra_argstrs=[('version', '1.0')],
        )

        call_args = mock_run.call_args[0][0]
        cmd_str = ' '.join(call_args)
        assert '--argstr version 1.0' in cmd_str

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_handles_no_lock_file(self, mock_run, mock_nix_dir, mock_system, tmp_path):
        """Test evaluation works without a lock file."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='"test"',
            stderr='',
        )

        flake_dir = tmp_path / 'flake'
        flake_dir.mkdir()
        # No flake.lock file

        result = run_nix_eval(
            flake_dir=flake_dir,
            attr='test',
        )

        assert result == '"test"'
        # Check that a default lock expression is used
        call_args = mock_run.call_args[0][0]
        expr_idx = call_args.index('--expr') + 1
        expr = call_args[expr_idx]
        assert 'nodes = { root = { inputs = {}; }; }' in expr

    @mock.patch('trix.nix.get_system')
    @mock.patch('trix.nix.get_nix_dir')
    @mock.patch('subprocess.run')
    def test_exits_on_failure(self, mock_run, mock_nix_dir, mock_system, tmp_path):
        """Test that sys.exit is called on nix-instantiate failure."""
        mock_system.return_value = 'x86_64-linux'
        mock_nix_dir.return_value = Path('/nix/store/fake-nix-dir')
        mock_run.return_value = mock.Mock(
            returncode=1,
            stdout='',
            stderr="error: attribute 'foo' not found",
        )

        flake_dir = tmp_path / 'flake'
        flake_dir.mkdir()
        (flake_dir / 'flake.lock').write_text('{"nodes":{},"root":"root","version":7}')

        with pytest.raises(SystemExit) as exc_info:
            run_nix_eval(
                flake_dir=flake_dir,
                attr='nonexistent',
            )

        assert exc_info.value.code == 1
