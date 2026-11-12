"""Shared publication fixture for remaining integration tests."""
import base64
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.parse

from support.fixture import BINARY, Fixture
from repository_accounts import Accounts, ALICE, BOB, ENV


class PublishFixture(Accounts):
    def __init__(self, root):
        super().__init__(root)
        gh = root/'bin/gh'
        original = root/'bin/gh-original'
        gh.rename(original)
        gh.write_text(f'#!{sys.executable}\n' + r'''import json,os,sys,time,urllib.parse
from pathlib import Path
args=sys.argv[1:]
endpoint=args[-1] if args else ''
if args[:1]!=['api'] or not endpoint.startswith('/user/orgs?'):
 os.execv(str(Path(__file__).with_name('gh-original')),[str(Path(__file__).with_name('gh-original')),*args])
c=json.loads(Path(os.environ['CONTEXT_SCENARIO']).read_text())
token=os.environ.get('GH_TOKEN','')
actors={value['token']:value for value in c['users'].values()}
actors[c['environment']['token']]=c['environment']
actor=actors.get(token)
assert actor is not None
assert all(os.environ.get(k)==token for k in ['GH_TOKEN','GITHUB_TOKEN','GH_ENTERPRISE_TOKEN','GITHUB_ENTERPRISE_TOKEN'])
page=int(urllib.parse.parse_qs(urllib.parse.urlsplit(endpoint).query)['page'][0])
with open(os.environ['CONTEXT_GH_LOG'],'a') as log:
 log.write(json.dumps({'kind':'orgs','page':page,'actor':actor['login'],'pinned':True,'args':args})+'\n')
if c.get('delay_org_page')==page:
 Path(os.environ['CONTEXT_GH_STARTED']).write_text('started')
 deadline=time.monotonic()+25
 while not Path(os.environ['CONTEXT_GH_RELEASE']).exists() and time.monotonic()<deadline:time.sleep(.02)
rows=c.get('organizations',{}).get(actor['login'],[])
if c.get('org_fail_page')==page:
 sys.stderr.write('provider diagnostic '+token);sys.exit(1)
print(json.dumps(rows[(page-1)*100:page*100]))
''')
        gh.chmod(0o700)
        self.env['GH_CONFIG_DIR'] = str(root/'private-gh-config')
        self.env['GIT_CONFIG_NOSYSTEM'] = '1'
        self.env['GIT_CONFIG_GLOBAL'] = str(root/'isolated-global')

    def local(self, name, saved=True):
        fixture = Fixture(self.root/name)
        if saved:
            self.cli('repositories', 'add', fixture.path)
        return fixture

    def request(self, fixture, method, *, fail=False, timeout=25, **fields):
        arguments = [str(BINARY), '--config', str(self.registry), '-C', str(fixture.path), 'repo', '-']
        result = subprocess.run(arguments, input=json.dumps({'method':method, **fields}), env=self.env,
                                capture_output=True, text=True, timeout=timeout)
        self.assert_clean(result.stdout+result.stderr)
        if fail:
            assert result.returncode != 0, (method, result.stdout)
            return result
        assert result.returncode == 0, (method, result.stderr, result.stdout)
        return json.loads(result.stdout)

    def cli_publish(self, fixture, *args, fail=False):
        command = [str(BINARY), '--config', str(self.registry), '-C', str(fixture.path), 'publish', *args]
        result = subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=25)
        self.assert_clean(result.stdout+result.stderr)
        if fail:
            assert result.returncode != 0, (args, result.stdout)
            return result
        assert result.returncode == 0, (args, result.stderr)
        return json.loads(result.stdout)

    def assert_clean(self, value):
        secrets = [ALICE, BOB, ENV, self.data['users']['AliceCase']['token']]
        for secret in secrets:
            forms = [secret, base64.b64encode(secret.encode()).decode(), urllib.parse.quote(secret, safe='')]
            assert all(form not in value for form in forms if form), 'Credential appeared in output'


def snapshot(s, f):
    gitdir = f.path/'.git'
    return ((gitdir/'config').read_bytes(), (gitdir/'HEAD').read_bytes(),
            f.git('for-each-ref', '--format=%(refname) %(objectname)').stdout,
            (gitdir/'index').read_bytes() if (gitdir/'index').exists() else None,
            s.registry.read_bytes() if s.registry.exists() else None)
