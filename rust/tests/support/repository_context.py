"""Shared fixtures for the remaining offline repository service tests."""
import errno
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from support.fixture import BINARY, Fixture


FORK = {
    'nameWithOwner': 'fork/repo',
    'url': 'https://github.example/fork/repo',
    'isFork': True,
    'parent': {'nameWithOwner': 'parent/repo', 'url': 'https://github.example/parent/repo'},
}
NONFORK = {**FORK, 'isFork': False, 'parent': None}


def private(path, content):
    path.write_bytes(content if isinstance(content, bytes) else content.encode())
    path.chmod(0o600)


def hardlink(source, destination):
    if hasattr(os, 'link'):
        os.link(source, destination)
        return
    # Android's Python can omit os.link; use the ordinary libc API when present.
    import ctypes
    libc = ctypes.CDLL(None, use_errno=True)
    linkat = libc.linkat
    linkat.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_int]
    linkat.restype = ctypes.c_int
    if linkat(-100, os.fsencode(source), -100, os.fsencode(destination), 0) != 0:
        number = ctypes.get_errno()
        raise OSError(number, os.strerror(number))


def hardlinks_available(root):
    source, destination = root/'hardlink-source', root/'hardlink-destination'
    private(source, b'hardlink fixture')
    try:
        hardlink(source, destination)
        assert source.stat().st_nlink == destination.stat().st_nlink == 2
        return True
    except OSError as error:
        if error.errno not in [errno.EACCES, errno.EPERM, errno.ENOTSUP, errno.ENOSYS]:
            raise
        print('SKIP: hardlink fixture unavailable on this filesystem (OS error '+str(error.errno)+')', flush=True)
        return False
    finally:
        source.unlink(missing_ok=True); destination.unlink(missing_ok=True)


def wait_for(predicate, label, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.02)
    raise AssertionError(label)


def stop(child):
    if child.poll() is None:
        child.send_signal(signal.SIGINT)
        try:
            child.communicate(timeout=8)
        except subprocess.TimeoutExpired:
            child.kill(); child.communicate(timeout=5)


class Suite:
    def __init__(self, root):
        self.root = root
        self.scenario = root/'gh-response.json'
        self.log = root/'gh-calls.jsonl'
        self.started = root/'gh-started'
        self.release = root/'gh-release'
        tools = root/'bin'; tools.mkdir()
        gh = tools/'gh'
        gh.write_text(f'#!{sys.executable}\n' + '''import json,os,sys,time
from pathlib import Path
scenario=json.loads(Path(os.environ['CONTEXT_SCENARIO']).read_text())
with open(os.environ['CONTEXT_GH_LOG'],'a') as log: log.write(json.dumps(sys.argv[1:])+'\\n')
if scenario.get('delay'):
    Path(os.environ['CONTEXT_GH_STARTED']).write_text('started')
    deadline=time.monotonic()+25
    while not Path(os.environ['CONTEXT_GH_RELEASE']).exists() and time.monotonic()<deadline:time.sleep(.02)
if scenario.get('stderr'):sys.stderr.write(scenario['stderr'])
if 'raw' in scenario:sys.stdout.write(scenario['raw'])
else:print(json.dumps(scenario.get('data')))
sys.exit(scenario.get('exit',0))
''')
        gh.chmod(0o700)
        self.env = {**os.environ,
                    'PATH': str(tools)+os.pathsep+os.environ['PATH'],
                    'CONTEXT_SCENARIO': str(self.scenario), 'CONTEXT_GH_LOG': str(self.log),
                    'CONTEXT_GH_STARTED': str(self.started), 'CONTEXT_GH_RELEASE': str(self.release)}
        self.respond()

    def respond(self, data=None, **extra):
        self.started.unlink(missing_ok=True); self.release.unlink(missing_ok=True)
        private(self.scenario, json.dumps({'data': FORK if data is None else data, **extra}))

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def seed(self, name, *options, remotes=True):
        f = Fixture(self.root/name, *options)
        (f.path/'file').write_bytes(b'base\n'); f.commit()
        for branch in ['fork-main', 'parent-main', 'feature']:
            f.git('branch', branch)
        if remotes:
            tip = f.git('rev-parse', 'HEAD').stdout.strip().decode()
            for remote, owner, default in [('origin', 'fork', 'fork-main'), ('upstream', 'parent', 'parent-main')]:
                f.git('remote', 'add', remote, f'https://github.example/{owner}/repo.git')
                f.git('update-ref', f'refs/remotes/{remote}/{default}', tip)
                f.git('symbolic-ref', f'refs/remotes/{remote}/HEAD', f'refs/remotes/{remote}/{default}')
                f.git('branch', '--set-upstream-to', f'{remote}/{default}', default)
        return f

    def command(self, f, *args, fail=False):
        result = subprocess.run([str(BINARY), '-C', str(f.path), *args], env=self.env,
                                capture_output=True, text=True, timeout=20)
        if fail:
            assert result.returncode, (args, result.stdout)
            return result
        assert result.returncode == 0, (args, result.stderr)
        return json.loads(result.stdout)

    def rpc(self, f, method, fail=False, **params):
        result = subprocess.run([str(BINARY), '-C', str(f.path), 'repo', '-'],
                                input=json.dumps({'method': method, **params}), env=self.env,
                                capture_output=True, text=True, timeout=20)
        if fail:
            assert result.returncode, (method, result.stdout)
            return result
        assert result.returncode == 0, (method, result.stderr)
        return json.loads(result.stdout)

    def context(self, f): return self.rpc(f, 'repository_context')
    def refresh(self, f, **kwargs): return self.rpc(f, 'refresh_repository_context', **kwargs)
    def target(self, f, target, basis=None, **kwargs):
        return self.rpc(f, 'set_fork_contribution_target', target=target, basis=basis, **kwargs)
    def policy(self, f): return self.rpc(f, 'branch_policy')
    def files(self, f):
        common = Path(f.git('rev-parse', '--path-format=absolute', '--git-common-dir').stdout.decode().strip())
        return common/'desktop-repository.json', common/'desktop-repository.lock'

    def marker(self, f): return self.files(f)[0].with_name('desktop-repository.id')

    def delayed(self, f):
        self.respond(delay=True)
        child = subprocess.Popen([str(BINARY), '-C', str(f.path), 'repository-context', '--refresh'],
                                 env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            wait_for(self.started.exists, 'gh metadata read did not start')
            return child
        except BaseException:
            stop(child)
            raise
