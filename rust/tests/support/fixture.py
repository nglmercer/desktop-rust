"""Shared Git repository fixture for remaining legacy integration tests."""
import json
from pathlib import Path
import subprocess

BINARY = Path(__file__).resolve().parents[2] / 'target/debug/desktop-rs'


class Fixture:
    def __init__(self, path, *options):
        self.path = path
        subprocess.run(['git', 'init', '-q', '-b', 'main', *options, str(path)], check=True)
        self.git('config', 'user.name', 'Discard Edge Test'); self.git('config', 'user.email', 'edges@example.invalid')
        self.git('config', 'commit.gpgsign', 'false')
    def git(self, *args, check=True):
        return subprocess.run(['git', '-C', str(self.path), *args], capture_output=True, check=check)
    def rpc(self, method, fail=False, **params):
        result = subprocess.run([str(BINARY), '-C', str(self.path), 'repo', '-'], input=json.dumps({'method': method, **params}), capture_output=True, text=True)
        if fail:
            assert result.returncode, (method, result.stdout)
            return result
        assert result.returncode == 0, (method, result.stderr)
        return json.loads(result.stdout)
    def discard(self, path, scope='all_changes', selection=None):
        plan = self.rpc('discard_preview', path=path, scope=scope, selection=selection)
        value = self.rpc('discard', path=path, scope=scope, selection=selection, basis=plan['basis'])
        return value['undo_id']
    def commit(self): self.git('add', '--all'); self.git('commit', '-qm', 'checkpoint')
    def undo(self, id, fail=False): return self.rpc('undo_discard', undo_id=id, fail=fail)
