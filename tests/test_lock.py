"""Tests for lock file handling."""

import json
from unittest import mock

from trix.lock import (
    _fetch_source_flake_lock,
    _format_locked_url,
    _lock_data_equal,
    _lock_input,
    _prefetch_flake,
    _read_lock,
    _write_lock,
)


class TestFormatLockedUrl:
    """Tests for _format_locked_url function."""

    def test_format_github_with_date(self):
        """Test formatting github node with lastModified."""
        node = {
            'locked': {
                'type': 'github',
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'rev': 'abc123def456',
                'lastModified': 1700000000,  # 2023-11-14
            }
        }
        result = _format_locked_url(node)
        assert result == 'github:NixOS/nixpkgs/abc123def456 (2023-11-14)'

    def test_format_github_without_date(self):
        """Test formatting github node without lastModified."""
        node = {
            'locked': {
                'type': 'github',
                'owner': 'tvbeat',
                'repo': 'ae',
                'rev': 'deadbeef',
            }
        }
        result = _format_locked_url(node)
        assert result == 'github:tvbeat/ae/deadbeef'

    def test_format_git_with_date(self):
        """Test formatting git node with lastModified."""
        node = {
            'locked': {
                'type': 'git',
                'url': 'https://example.com/repo.git',
                'rev': 'abc123',
                'lastModified': 1700000000,
            }
        }
        result = _format_locked_url(node)
        assert result == 'git+https://example.com/repo.git?rev=abc123 (2023-11-14)'

    def test_format_git_without_date(self):
        """Test formatting git node without lastModified."""
        node = {
            'locked': {
                'type': 'git',
                'url': 'https://example.com/repo.git',
                'rev': 'abc123',
            }
        }
        result = _format_locked_url(node)
        assert result == 'git+https://example.com/repo.git?rev=abc123'

    def test_format_path(self):
        """Test formatting path node."""
        node = {
            'locked': {
                'type': 'path',
                'path': '/home/user/my-flake',
            }
        }
        result = _format_locked_url(node)
        assert result == 'path:/home/user/my-flake'

    def test_format_follows_single(self):
        """Test formatting follows pseudo-node with single element."""
        node = {'_follows': ['nixpkgs']}
        result = _format_locked_url(node)
        assert result == "follows 'nixpkgs'"

    def test_format_follows_nested(self):
        """Test formatting follows pseudo-node with nested path."""
        node = {'_follows': ['ae', 'nixpkgs']}
        result = _format_locked_url(node)
        assert result == "follows 'ae/nixpkgs'"

    def test_format_unknown_type(self):
        """Test formatting node with unknown type."""
        node = {
            'locked': {
                'type': 'tarball',
                'url': 'https://example.com/archive.tar.gz',
            }
        }
        result = _format_locked_url(node)
        # Falls back to str(locked)
        assert 'tarball' in result

    def test_format_empty_node(self):
        """Test formatting empty node."""
        node = {}
        result = _format_locked_url(node)
        assert result == '{}'


class TestLockDataEqual:
    """Tests for _lock_data_equal function."""

    def test_equal_simple(self):
        """Test equal simple dicts."""
        a = {'version': 7, 'root': 'root'}
        b = {'version': 7, 'root': 'root'}
        assert _lock_data_equal(a, b) is True

    def test_equal_different_key_order(self):
        """Test equal dicts with different key order."""
        a = {'version': 7, 'root': 'root', 'nodes': {}}
        b = {'nodes': {}, 'root': 'root', 'version': 7}
        assert _lock_data_equal(a, b) is True

    def test_equal_nested(self):
        """Test equal nested dicts."""
        a = {
            'nodes': {
                'root': {'inputs': {'nixpkgs': 'nixpkgs'}},
                'nixpkgs': {'locked': {'rev': 'abc123'}},
            },
            'root': 'root',
            'version': 7,
        }
        b = {
            'version': 7,
            'root': 'root',
            'nodes': {
                'nixpkgs': {'locked': {'rev': 'abc123'}},
                'root': {'inputs': {'nixpkgs': 'nixpkgs'}},
            },
        }
        assert _lock_data_equal(a, b) is True

    def test_not_equal_different_value(self):
        """Test unequal dicts with different value."""
        a = {'version': 7}
        b = {'version': 8}
        assert _lock_data_equal(a, b) is False

    def test_not_equal_extra_key(self):
        """Test unequal dicts with extra key."""
        a = {'version': 7}
        b = {'version': 7, 'extra': 'key'}
        assert _lock_data_equal(a, b) is False

    def test_not_equal_nested_difference(self):
        """Test unequal nested dicts."""
        a = {'nodes': {'nixpkgs': {'locked': {'rev': 'abc123'}}}}
        b = {'nodes': {'nixpkgs': {'locked': {'rev': 'def456'}}}}
        assert _lock_data_equal(a, b) is False

    def test_equal_with_lists(self):
        """Test equal dicts containing lists (follows references)."""
        a = {'inputs': {'nixpkgs': ['ae', 'nixpkgs']}}
        b = {'inputs': {'nixpkgs': ['ae', 'nixpkgs']}}
        assert _lock_data_equal(a, b) is True

    def test_not_equal_different_list(self):
        """Test unequal dicts with different list values."""
        a = {'inputs': {'nixpkgs': ['ae', 'nixpkgs']}}
        b = {'inputs': {'nixpkgs': ['nixpkgs']}}
        assert _lock_data_equal(a, b) is False

    def test_equal_empty(self):
        """Test equal empty dicts."""
        assert _lock_data_equal({}, {}) is True

    def test_equal_realistic_lock(self):
        """Test with realistic lock file structure."""
        lock_a = {
            'nodes': {
                'root': {'inputs': {'ae': 'ae'}},
                'ae': {
                    'locked': {
                        'lastModified': 1765562251,
                        'narHash': 'sha256-abc',
                        'owner': 'tvbeat',
                        'repo': 'ae',
                        'rev': '31e82430588331991a690eb20d545eb5bc40d38d',
                        'type': 'github',
                    },
                    'original': {'owner': 'tvbeat', 'repo': 'ae', 'type': 'github'},
                    'inputs': {
                        'nixpkgs': 'nixpkgs',
                        'flake-utils': 'flake-utils',
                    },
                },
            },
            'root': 'root',
            'version': 7,
        }
        # Same content, different key order
        lock_b = {
            'version': 7,
            'root': 'root',
            'nodes': {
                'ae': {
                    'inputs': {
                        'flake-utils': 'flake-utils',
                        'nixpkgs': 'nixpkgs',
                    },
                    'original': {'type': 'github', 'owner': 'tvbeat', 'repo': 'ae'},
                    'locked': {
                        'type': 'github',
                        'rev': '31e82430588331991a690eb20d545eb5bc40d38d',
                        'repo': 'ae',
                        'owner': 'tvbeat',
                        'narHash': 'sha256-abc',
                        'lastModified': 1765562251,
                    },
                },
                'root': {'inputs': {'ae': 'ae'}},
            },
        }
        assert _lock_data_equal(lock_a, lock_b) is True


class TestReadLock:
    """Tests for _read_lock function."""

    def test_read_existing_lock(self, tmp_path):
        """Test reading an existing lock file."""
        lock_file = tmp_path / 'flake.lock'
        lock_data = {
            'nodes': {
                'root': {'inputs': {'nixpkgs': 'nixpkgs'}},
                'nixpkgs': {'locked': {'rev': 'abc123'}},
            },
            'root': 'root',
            'version': 7,
        }
        lock_file.write_text(json.dumps(lock_data))

        result = _read_lock(lock_file)
        assert result == lock_data

    def test_read_nonexistent_returns_empty_structure(self, tmp_path):
        """Test reading nonexistent file returns empty structure."""
        lock_file = tmp_path / 'flake.lock'

        result = _read_lock(lock_file)
        assert result == {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 7,
        }

    def test_read_invalid_json_returns_empty_structure(self, tmp_path):
        """Test reading invalid JSON returns empty structure."""
        lock_file = tmp_path / 'flake.lock'
        lock_file.write_text('not valid json {{{')

        result = _read_lock(lock_file)
        assert result == {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 7,
        }

    def test_read_empty_file_returns_empty_structure(self, tmp_path):
        """Test reading empty file returns empty structure."""
        lock_file = tmp_path / 'flake.lock'
        lock_file.write_text('')

        result = _read_lock(lock_file)
        assert result == {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 7,
        }

    def test_read_version_7_no_warning(self, tmp_path, capsys):
        """Test that version 7 lock files don't produce warnings."""
        lock_file = tmp_path / 'flake.lock'
        lock_data = {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 7,
        }
        lock_file.write_text(json.dumps(lock_data))

        _read_lock(lock_file)

        captured = capsys.readouterr()
        assert 'warning:' not in captured.err

    def test_read_version_8_warns(self, tmp_path, capsys):
        """Test that non-v7 lock files produce a warning."""
        lock_file = tmp_path / 'flake.lock'
        lock_data = {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 8,
        }
        lock_file.write_text(json.dumps(lock_data))

        result = _read_lock(lock_file)

        # Should still return the data
        assert result == lock_data
        # But should warn
        captured = capsys.readouterr()
        assert 'warning:' in captured.err
        assert 'version 8' in captured.err

    def test_read_version_6_warns(self, tmp_path, capsys):
        """Test that older lock file versions produce a warning."""
        lock_file = tmp_path / 'flake.lock'
        lock_data = {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 6,
        }
        lock_file.write_text(json.dumps(lock_data))

        result = _read_lock(lock_file)

        # Should still return the data
        assert result == lock_data
        # But should warn
        captured = capsys.readouterr()
        assert 'warning:' in captured.err
        assert 'version 6' in captured.err


class TestWriteLock:
    """Tests for _write_lock function."""

    def test_write_creates_file(self, tmp_path):
        """Test writing creates a new file."""
        lock_file = tmp_path / 'flake.lock'
        lock_data = {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 7,
        }

        _write_lock(lock_file, lock_data)

        assert lock_file.exists()
        content = json.loads(lock_file.read_text())
        assert content == lock_data

    def test_write_overwrites_existing(self, tmp_path):
        """Test writing overwrites existing file."""
        lock_file = tmp_path / 'flake.lock'
        lock_file.write_text('{"old": "data"}')

        new_data = {
            'nodes': {'root': {'inputs': {'new': 'input'}}},
            'root': 'root',
            'version': 7,
        }

        _write_lock(lock_file, new_data)

        content = json.loads(lock_file.read_text())
        assert content == new_data

    def test_write_formats_with_indent(self, tmp_path):
        """Test that output is formatted with indent."""
        lock_file = tmp_path / 'flake.lock'
        lock_data = {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 7,
        }

        _write_lock(lock_file, lock_data)

        content = lock_file.read_text()
        # Should have newlines (formatted)
        assert '\n' in content
        # Should have indentation
        assert '  ' in content

    def test_write_ends_with_newline(self, tmp_path):
        """Test that output ends with newline."""
        lock_file = tmp_path / 'flake.lock'
        lock_data = {'version': 7}

        _write_lock(lock_file, lock_data)

        content = lock_file.read_text()
        assert content.endswith('\n')

    def test_roundtrip(self, tmp_path):
        """Test write then read returns same data."""
        lock_file = tmp_path / 'flake.lock'
        lock_data = {
            'nodes': {
                'root': {'inputs': {'nixpkgs': 'nixpkgs', 'ae': 'ae'}},
                'nixpkgs': {
                    'locked': {
                        'type': 'github',
                        'owner': 'NixOS',
                        'repo': 'nixpkgs',
                        'rev': 'abc123',
                        'narHash': 'sha256-xyz',
                        'lastModified': 1700000000,
                    },
                    'original': {'type': 'github', 'owner': 'NixOS', 'repo': 'nixpkgs'},
                },
            },
            'root': 'root',
            'version': 7,
        }

        _write_lock(lock_file, lock_data)
        result = _read_lock(lock_file)

        assert result == lock_data


class TestPrefetchFlake:
    """Tests for _prefetch_flake function."""

    @mock.patch('subprocess.run')
    def test_prefetch_success(self, mock_run):
        """Test successful prefetch."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout=json.dumps(
                {
                    'hash': 'sha256-abc123',
                    'locked': {
                        'type': 'github',
                        'owner': 'NixOS',
                        'repo': 'nixpkgs',
                        'rev': 'deadbeef',
                        'lastModified': 1700000000,
                    },
                    'original': {
                        'type': 'github',
                        'owner': 'NixOS',
                        'repo': 'nixpkgs',
                    },
                }
            ),
        )

        result = _prefetch_flake('github:NixOS/nixpkgs')

        assert result is not None
        assert result['hash'] == 'sha256-abc123'
        assert result['locked']['rev'] == 'deadbeef'

    @mock.patch('subprocess.run')
    def test_prefetch_failure_returns_none(self, mock_run):
        """Test that failure returns None."""
        mock_run.return_value = mock.Mock(
            returncode=1,
            stderr='error: failed to fetch',
        )

        result = _prefetch_flake('github:nonexistent/repo')

        assert result is None

    @mock.patch('subprocess.run')
    def test_prefetch_invalid_json_returns_none(self, mock_run):
        """Test that invalid JSON returns None."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='not json',
        )

        result = _prefetch_flake('github:NixOS/nixpkgs')

        assert result is None

    @mock.patch('subprocess.run')
    def test_prefetch_uses_correct_command(self, mock_run):
        """Test that correct nix command is used."""
        mock_run.return_value = mock.Mock(
            returncode=0,
            stdout='{}',
        )

        _prefetch_flake('github:NixOS/nixpkgs')

        call_args = mock_run.call_args[0][0]
        assert 'nix' in call_args
        assert 'flake' in call_args
        assert 'prefetch' in call_args
        assert 'github:NixOS/nixpkgs' in call_args
        assert '--json' in call_args


class TestLockInput:
    """Tests for _lock_input function."""

    @mock.patch('trix.lock._prefetch_flake')
    def test_lock_github_input(self, mock_prefetch):
        """Test locking a github input."""
        mock_prefetch.return_value = {
            'hash': 'sha256-abc123',
            'locked': {
                'type': 'github',
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'rev': 'deadbeef123',
                'lastModified': 1700000000,
            },
            'original': {
                'type': 'github',
                'owner': 'NixOS',
                'repo': 'nixpkgs',
            },
        }

        spec = {'type': 'github', 'owner': 'NixOS', 'repo': 'nixpkgs'}
        result = _lock_input('nixpkgs', spec)

        assert result is not None
        assert result['locked']['type'] == 'github'
        assert result['locked']['owner'] == 'NixOS'
        assert result['locked']['repo'] == 'nixpkgs'
        assert result['locked']['rev'] == 'deadbeef123'
        assert result['locked']['narHash'] == 'sha256-abc123'
        assert result['original']['type'] == 'github'

    @mock.patch('trix.lock._prefetch_flake')
    def test_lock_github_with_ref(self, mock_prefetch):
        """Test locking a github input with ref."""
        mock_prefetch.return_value = {
            'hash': 'sha256-xyz',
            'locked': {
                'type': 'github',
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'rev': 'abc123',
                'lastModified': 1700000000,
            },
            'original': {
                'type': 'github',
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'ref': 'nixos-unstable',
            },
        }

        spec = {'type': 'github', 'owner': 'NixOS', 'repo': 'nixpkgs', 'ref': 'nixos-unstable'}
        result = _lock_input('nixpkgs', spec)

        assert result is not None
        assert result['original'].get('ref') == 'nixos-unstable'

    @mock.patch('trix.lock._prefetch_flake')
    def test_lock_github_non_flake(self, mock_prefetch):
        """Test locking a non-flake github input."""
        mock_prefetch.return_value = {
            'hash': 'sha256-abc',
            'locked': {'type': 'github', 'owner': 'foo', 'repo': 'bar', 'rev': '123'},
            'original': {'type': 'github', 'owner': 'foo', 'repo': 'bar'},
        }

        spec = {'type': 'github', 'owner': 'foo', 'repo': 'bar', 'flake': False}
        result = _lock_input('bar', spec)

        assert result is not None
        assert result.get('flake') is False

    @mock.patch('trix.lock._prefetch_flake')
    def test_lock_git_input(self, mock_prefetch):
        """Test locking a git input."""
        mock_prefetch.return_value = {
            'hash': 'sha256-git123',
            'locked': {
                'type': 'git',
                'url': 'https://example.com/repo.git',
                'rev': 'abc123',
                'lastModified': 1700000000,
            },
            'original': {
                'type': 'git',
                'url': 'https://example.com/repo.git',
            },
        }

        spec = {'type': 'git', 'url': 'https://example.com/repo.git'}
        result = _lock_input('myrepo', spec)

        assert result is not None
        assert result['locked']['type'] == 'git'
        assert result['locked']['url'] == 'https://example.com/repo.git'

    def test_lock_path_input(self):
        """Test locking a path input."""
        spec = {'type': 'path', 'path': '/home/user/flake'}
        result = _lock_input('local', spec)

        assert result is not None
        assert result['locked']['type'] == 'path'
        assert result['locked']['path'] == '/home/user/flake'
        assert result['original']['path'] == '/home/user/flake'

    def test_lock_nix_path_returns_none(self):
        """Test that nix-path inputs return None (not locked)."""
        spec = {'type': 'nix-path', 'name': 'nixpkgs'}
        result = _lock_input('nixpkgs', spec)

        assert result is None

    def test_lock_unknown_type_returns_none(self):
        """Test that unknown input types return None."""
        spec = {'type': 'unknown', 'url': 'something:weird'}
        result = _lock_input('weird', spec)

        assert result is None

    @mock.patch('trix.lock._prefetch_flake')
    def test_lock_prefetch_failure_returns_none(self, mock_prefetch):
        """Test that prefetch failure returns None."""
        mock_prefetch.return_value = None

        spec = {'type': 'github', 'owner': 'nonexistent', 'repo': 'repo'}
        result = _lock_input('repo', spec)

        assert result is None


class TestFetchSourceFlakeLock:
    """Tests for _fetch_source_flake_lock function."""

    def test_path_input_reads_lock_file(self, tmp_path):
        """Test that path inputs read flake.lock directly from filesystem."""
        # Create a mock flake.lock in the path
        lock_data = {
            'nodes': {'root': {'inputs': {'dep': 'dep'}}, 'dep': {'locked': {'type': 'github'}}},
            'root': 'root',
            'version': 7,
        }
        lock_file = tmp_path / 'flake.lock'
        lock_file.write_text(json.dumps(lock_data))

        node = {'locked': {'type': 'path', 'path': str(tmp_path)}}
        result = _fetch_source_flake_lock(node)

        assert result == lock_data

    def test_path_input_no_lock_file_returns_none(self, tmp_path):
        """Test that path inputs without flake.lock return None."""
        node = {'locked': {'type': 'path', 'path': str(tmp_path)}}
        result = _fetch_source_flake_lock(node)

        assert result is None

    def test_path_input_empty_path_returns_none(self):
        """Test that path inputs with empty path return None."""
        node = {'locked': {'type': 'path', 'path': ''}}
        result = _fetch_source_flake_lock(node)

        assert result is None

    def test_path_input_invalid_json_returns_none(self, tmp_path):
        """Test that path inputs with invalid JSON return None."""
        lock_file = tmp_path / 'flake.lock'
        lock_file.write_text('not valid json')

        node = {'locked': {'type': 'path', 'path': str(tmp_path)}}
        result = _fetch_source_flake_lock(node)

        assert result is None

    @mock.patch('subprocess.run')
    def test_github_input_uses_correct_url(self, mock_run):
        """Test that github inputs construct correct archive URL."""
        mock_run.return_value = mock.Mock(returncode=0, stdout='""')

        node = {
            'locked': {
                'type': 'github',
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'rev': 'abc123',
                'narHash': 'sha256-xyz',
            }
        }
        _fetch_source_flake_lock(node)

        call_args = mock_run.call_args[0][0]
        expr = ' '.join(call_args)
        assert 'https://github.com/NixOS/nixpkgs/archive/abc123.tar.gz' in expr

    @mock.patch('subprocess.run')
    def test_gitlab_input_uses_correct_url(self, mock_run):
        """Test that gitlab inputs construct correct archive URL."""
        mock_run.return_value = mock.Mock(returncode=0, stdout='""')

        node = {
            'locked': {
                'type': 'gitlab',
                'owner': 'mygroup',
                'repo': 'myrepo',
                'rev': 'def456',
                'narHash': 'sha256-xyz',
            }
        }
        _fetch_source_flake_lock(node)

        call_args = mock_run.call_args[0][0]
        expr = ' '.join(call_args)
        assert 'https://gitlab.com/mygroup/myrepo/-/archive/def456/myrepo-def456.tar.gz' in expr

    @mock.patch('subprocess.run')
    def test_gitlab_self_hosted_uses_custom_host(self, mock_run):
        """Test that self-hosted gitlab uses custom host."""
        mock_run.return_value = mock.Mock(returncode=0, stdout='""')

        node = {
            'locked': {
                'type': 'gitlab',
                'owner': 'mygroup',
                'repo': 'myrepo',
                'rev': 'def456',
                'narHash': 'sha256-xyz',
                'host': 'gitlab.example.com',
            }
        }
        _fetch_source_flake_lock(node)

        call_args = mock_run.call_args[0][0]
        expr = ' '.join(call_args)
        assert 'https://gitlab.example.com/mygroup/myrepo/-/archive/def456/myrepo-def456.tar.gz' in expr

    @mock.patch('subprocess.run')
    def test_sourcehut_input_uses_correct_url(self, mock_run):
        """Test that sourcehut inputs construct correct archive URL."""
        mock_run.return_value = mock.Mock(returncode=0, stdout='""')

        node = {
            'locked': {
                'type': 'sourcehut',
                'owner': 'sircmpwn',
                'repo': 'hare',
                'rev': 'ghi789',
                'narHash': 'sha256-xyz',
            }
        }
        _fetch_source_flake_lock(node)

        call_args = mock_run.call_args[0][0]
        expr = ' '.join(call_args)
        assert 'https://git.sr.ht/~sircmpwn/hare/archive/ghi789.tar.gz' in expr

    @mock.patch('subprocess.run')
    def test_git_input_uses_fetchgit(self, mock_run):
        """Test that git inputs use builtins.fetchGit."""
        mock_run.return_value = mock.Mock(returncode=0, stdout='""')

        node = {
            'locked': {
                'type': 'git',
                'url': 'https://example.com/repo.git',
                'rev': 'jkl012',
                'narHash': 'sha256-xyz',
            }
        }
        _fetch_source_flake_lock(node)

        call_args = mock_run.call_args[0][0]
        expr = ' '.join(call_args)
        assert 'builtins.fetchGit' in expr
        assert 'https://example.com/repo.git' in expr

    @mock.patch('subprocess.run')
    def test_file_input_uses_fetchtarball(self, mock_run):
        """Test that file inputs use builtins.fetchTarball."""
        mock_run.return_value = mock.Mock(returncode=0, stdout='""')

        node = {
            'locked': {
                'type': 'file',
                'url': 'file:///path/to/archive.tar.gz',
                'narHash': 'sha256-xyz',
            }
        }
        _fetch_source_flake_lock(node)

        call_args = mock_run.call_args[0][0]
        expr = ' '.join(call_args)
        assert 'builtins.fetchTarball' in expr
        assert 'file:///path/to/archive.tar.gz' in expr

    def test_unknown_type_returns_none(self):
        """Test that unknown source types return None."""
        node = {'locked': {'type': 'unknown', 'url': 'something'}}
        result = _fetch_source_flake_lock(node)

        assert result is None

    def test_unknown_type_warns(self, capsys):
        """Test that unknown source types produce a warning."""
        node = {'locked': {'type': 'weirdtype', 'url': 'something'}}
        result = _fetch_source_flake_lock(node, input_name='myinput')

        assert result is None
        captured = capsys.readouterr()
        assert 'warning:' in captured.err
        assert 'weirdtype' in captured.err
        assert 'myinput' in captured.err

    def test_mercurial_returns_none(self):
        """Test that mercurial inputs return None (not supported)."""
        node = {'locked': {'type': 'mercurial', 'url': 'https://example.com/hg'}}
        result = _fetch_source_flake_lock(node)

        assert result is None

    def test_mercurial_warns(self, capsys):
        """Test that mercurial inputs produce a warning."""
        node = {'locked': {'type': 'mercurial', 'url': 'https://example.com/hg'}}
        result = _fetch_source_flake_lock(node, input_name='myhgrepo')

        assert result is None
        captured = capsys.readouterr()
        assert 'warning:' in captured.err
        assert 'mercurial' in captured.err
        assert 'myhgrepo' in captured.err

    def test_hg_type_warns(self, capsys):
        """Test that hg inputs (alias for mercurial) produce a warning."""
        node = {'locked': {'type': 'hg', 'url': 'https://example.com/hg'}}
        result = _fetch_source_flake_lock(node, input_name='hgrepo')

        assert result is None
        captured = capsys.readouterr()
        assert 'warning:' in captured.err
        assert 'mercurial' in captured.err
        assert 'hgrepo' in captured.err

    @mock.patch('subprocess.run')
    def test_nix_failure_returns_none(self, mock_run):
        """Test that nix-instantiate failure returns None."""
        mock_run.return_value = mock.Mock(returncode=1, stderr='error')

        node = {
            'locked': {
                'type': 'github',
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'rev': 'abc123',
                'narHash': 'sha256-xyz',
            }
        }
        result = _fetch_source_flake_lock(node)

        assert result is None

    @mock.patch('subprocess.run')
    def test_parses_lock_content(self, mock_run):
        """Test that lock file content is properly parsed."""
        lock_data = {
            'nodes': {'root': {'inputs': {}}},
            'root': 'root',
            'version': 7,
        }
        # nix-instantiate returns quoted, escaped JSON
        escaped_json = json.dumps(json.dumps(lock_data))
        mock_run.return_value = mock.Mock(returncode=0, stdout=escaped_json)

        node = {
            'locked': {
                'type': 'github',
                'owner': 'NixOS',
                'repo': 'nixpkgs',
                'rev': 'abc123',
                'narHash': 'sha256-xyz',
            }
        }
        result = _fetch_source_flake_lock(node)

        assert result == lock_data
