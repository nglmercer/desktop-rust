"""Shared account fixtures for the remaining Python integration suites."""
import copy
import json
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import sys
from contextlib import contextmanager
from support.fixture import BINARY
from support.repository_context import Suite, private, stop, wait_for
from support.bridge import WebSocket

ALICE = 'synthetic-assigned-alice-private'
BOB = 'synthetic-assigned-bob-private'
ENV = 'synthetic-effective-environment-private'
SECRET = 'synthetic-account-diagnostic-private'
NAMED = {'mode': 'assigned', 'host': 'github.com', 'login': 'AliceCase'}
OTHER = {**NAMED, 'login': 'BobCase'}
NONE = {'mode': 'none'}
EFFECTIVE = {'mode': 'effective'}


def clean(value):
    text = value if isinstance(value, str) else json.dumps(value)
    assert not any(secret in text for secret in [ALICE, BOB, ENV, SECRET]), text
    return value


def path_json(path):
    try:
        str(path).encode('utf-8')
        return str(path)
    except UnicodeEncodeError:
        return {'bytes': list(os.fsencode(path))}


class Accounts(Suite):
    def __init__(self, root):
        super().__init__(root)
        self.registry = root/'accounts/repositories.json'
        self.env.update(GH_TOKEN=ENV, GITHUB_TOKEN=BOB, GH_ENTERPRISE_TOKEN=ENV,
                        GITHUB_ENTERPRISE_TOKEN=BOB, GH_REPO='foreign.invalid/wrong/repo',
                        GH_HOST='foreign.invalid', GH_DEBUG='api', DEBUG='1')
        gh = root/'bin/gh'
        gh.write_text(f'#!{sys.executable}\n' + r'''import json,os,sys,time
from pathlib import Path
args=sys.argv[1:];c=json.loads(Path(os.environ['CONTEXT_SCENARIO']).read_text())
body=sys.stdin.buffer.read()
host=args[args.index('--hostname')+1] if '--hostname' in args else c['host']
kind='protocol' if args[:3]==['config','get','git_protocol'] else 'token' if args[:2]==['auth','token'] else 'catalog' if args[:2]==['auth','status'] else 'api' if args and args[0]=='api' else 'metadata' if args[:2]==['repo','view'] else 'typed'
login=args[args.index('--user')+1] if '--user' in args else None
variables=['GH_TOKEN','GITHUB_TOKEN'] if host=='github.com' or host.endswith('.ghe.com') else ['GH_ENTERPRISE_TOKEN','GITHUB_ENTERPRISE_TOKEN']
token=next((os.environ[key] for key in variables if os.environ.get(key)),c['users'][c['active']]['token'])
if kind=='token' and login is not None:token=c['users'].get(login,{}).get('token','')
actors={value['token']:value for value in c['users'].values()}
actors[c['environment']['token']]=c['environment']
actor=actors.get(token)
endpoint=('graphql' if 'graphql' in args else args[-1]) if kind=='api' else None
entry={'args':args,'kind':kind,'actor':None if actor is None else actor['login'],'body':body.decode('utf-8'),
       'tokens_absent':not any(key in os.environ for key in ['GH_TOKEN','GITHUB_TOKEN','GH_ENTERPRISE_TOKEN','GITHUB_ENTERPRISE_TOKEN']),
       'pinned':bool(actor) and all(os.environ.get(key)==token for key in ['GH_TOKEN','GITHUB_TOKEN','GH_ENTERPRISE_TOKEN','GITHUB_ENTERPRISE_TOKEN']),
       'debug_cleared':not any(key in os.environ for key in ['GH_DEBUG','DEBUG']),
       'defaults_cleared':not any(key in os.environ for key in ['GH_HOST','GH_REPO'])}
with open(os.environ['CONTEXT_GH_LOG'],'a') as log:log.write(json.dumps(entry)+'\n')
delay=c.get('delay')
if delay and delay['kind']==kind and (delay.get('endpoint') is None or delay['endpoint']==endpoint):
 Path(os.environ['CONTEXT_GH_STARTED']).write_text('started')
 deadline=time.monotonic()+25
 while not Path(os.environ['CONTEXT_GH_RELEASE']).exists() and time.monotonic()<deadline:time.sleep(.02)
if kind=='protocol':
 print(c.get('git_protocol','https'));sys.exit(0)
if kind=='catalog':
 if c.get('catalog_failure'):
  sys.stdout.write(c['secret']);sys.stderr.write(c['secret']);sys.exit(1)
 rows=[]
 for name in c['visible']:
  user=c['users'][name]
  rows.append({'host':host,'login':name,'active':name==c['active'],'state':'error' if name in c['signed_out'] else 'success','gitProtocol':'https','tokenSource':'keyring','oauthToken':c['secret'],'token':c['secret'],'unknown':{'token':c['secret']}})
 print(json.dumps({'hosts':{host:rows}}));sys.exit(0)
if kind=='token':
 if login is not None and (login not in c['visible'] or login in c['signed_out']):
  sys.stderr.write(c['secret']);sys.exit(1)
 if not token:sys.stderr.write(c['secret']);sys.exit(1)
 print(token);sys.exit(0)
if actor is None:sys.stderr.write(c['secret']);sys.exit(4)
if kind=='api':
 failure=c.get('api_failures',{}).get(endpoint)
 if failure:
  sys.stdout.write(c['secret']);sys.stderr.write(c['secret']);sys.exit(failure)
 if not entry['pinned']:sys.stderr.write(c['secret']);sys.exit(93)
 if endpoint=='graphql':
  payload=json.loads(body);query=payload['query'];repo=c['repositories'][actor['login']]
  if c.get('echo_credential'):sys.stderr.write('diagnostic '+token+'\n')
  if 'createPullRequest' in query:value={'data':{'createPullRequest':{'pullRequest':{'id':'PR_fixture','url':'https://'+c['host']+'/'+repo['full_name']+'/pull/7'}}}}
  else:value={'data':{'repository':{'id':'R_fixture','nameWithOwner':repo['full_name'],'url':repo['html_url'],'pullRequests':{'nodes':[],'pageInfo':{'hasNextPage':False,'endCursor':None}}}}}
 elif endpoint=='/user':value={'id':actor['id'],'login':actor['login'],'plan':{'name':'pro'}}
 elif endpoint in c.get('pull_requests',{}):value=c['pull_requests'][endpoint]
 elif endpoint in c.get('repository_responses',{}):value=c['repository_responses'][endpoint]
 elif endpoint.startswith('/repos/') and '/rules/' not in endpoint:value=c['repositories'][actor['login']]
 elif '/rules/branches/' in endpoint:value=[]
 else:sys.stderr.write(c['secret']);sys.exit(94)
 print(json.dumps(value));sys.exit(0)
if kind=='metadata':
 repo=c['repositories'][actor['login']]
 parent=repo.get('parent')
 print(json.dumps({'nameWithOwner':repo['full_name'],'url':repo['html_url'],'isFork':repo['fork'],'parent':None if parent is None else {'nameWithOwner':parent['full_name'],'url':parent['html_url']}}));sys.exit(0)
if c.get('echo_credential'):sys.stderr.write('diagnostic '+token+'\n')
if args[:2] in [['pr','create'],['issue','create']]:
 print('https://'+c['host']+'/fork/repo/'+('pull/7' if args[0]=='pr' else 'issues/8')+('?trace='+token if c.get('echo_credential') else ''))
elif '--json' in args:
 value=[{'number':8,'title':'offline issue','body':token if c.get('echo_credential') else 'safe'}] if args[1]=='list' else {'number':8,'title':'offline issue','body':token if c.get('echo_credential') else 'safe'}
 print(json.dumps(value))
else:print('offline typed operation')
sys.exit(c.get('typed_code',0))
''')
        gh.chmod(0o700)
        real_git = shutil.which('git'); assert real_git
        git = root/'bin/git'
        git.write_text(f'#!{sys.executable}\n' + f'''import subprocess,sys
args=sys.argv[1:];remaining=iter(args);command=None;tail=[]
for argument in remaining:
 if argument in ['-C','-c','--git-dir','--work-tree','--namespace']:next(remaining,None)
 elif not argument.startswith('-'):command=argument;tail=list(remaining);break
if command in ['fetch','push','pull','clone'] or (command=='ls-remote' and '--get-url' not in tail) or (command=='remote' and any(value in tail for value in ['update','prune','-f','--fetch'])):sys.exit(97)
result=subprocess.run([{real_git!r},*args]);sys.exit(result.returncode)
''')
        git.chmod(0o700)
        self.configure()

    def configure(self, **changes):
        host = changes.pop('host', 'github.com')
        base = {'id': 2001, 'full_name': 'fork/repo', 'name': 'repo', 'html_url': f'https://{host}/fork/repo',
                'owner': {'login': 'fork'}, 'private': True, 'fork': False, 'parent': None}
        self.data = {'host': host, 'active': 'BobCase', 'visible': ['AliceCase', 'BobCase'], 'signed_out': [],
                     'users': {'AliceCase': {'id': 101, 'login': 'alicecase', 'token': ALICE},
                               'BobCase': {'id': 202, 'login': 'bobcase', 'token': BOB}},
                     'environment': {'id': 303, 'login': 'environment-user', 'token': ENV},
                     'repositories': {'alicecase': {**base, 'fork': True, 'parent': {'full_name': 'parent/repo', 'html_url': f'https://{host}/parent/repo'}},
                                      'bobcase': copy.deepcopy(base), 'environment-user': copy.deepcopy(base)},
                     'secret': SECRET, **changes}
        self.started.unlink(missing_ok=True); self.release.unlink(missing_ok=True)
        private(self.scenario, json.dumps(self.data))

    def update(self, **changes):
        self.data.update(changes); private(self.scenario, json.dumps(self.data))

    def seed(self, name, *options, save=True):
        f = super().seed(name, *options)
        for remote, owner in [('origin', 'fork'), ('upstream', 'parent')]:
            f.git('remote', 'set-url', remote, f'https://{self.data["host"]}/{owner}/repo.git')
        if save: self.cli('repositories', 'add', f.path)
        return f

    def invoke(self, arguments, *, payload=None, fail=False, config=None, timeout=25):
        argv = [str(BINARY), '--config', str(self.registry if config is None else config), *map(str, arguments)]
        result = subprocess.run(argv, input=payload, env=self.env, capture_output=True, text=True, timeout=timeout)
        clean(result.stdout); clean(result.stderr)
        if fail:
            assert result.returncode, (arguments, result.stdout)
            return result
        assert result.returncode == 0, (arguments, result.stderr, result.stdout)
        return json.loads(result.stdout)

    def cli(self, *args, **kwargs): return self.invoke(args, **kwargs)
    def command(self, f, *args, **kwargs): return self.invoke(['-C', f.path, *args], **kwargs)
    def rpc(self, f, method, *, fail=False, config=None, **params):
        return self.command(f, 'repo', '-', payload=json.dumps({'method': method, **params}), fail=fail, config=config)
    def clear_calls(self): self.log.unlink(missing_ok=True)
    def account(self, f, **kwargs): return self.rpc(f, 'repository_account', **kwargs)
    def catalog(self, f, **kwargs): return self.rpc(f, 'repository_account_catalog', **kwargs)
    def preview(self, f, target=NAMED, **kwargs): return self.rpc(f, 'repository_account_preview', target=target, **kwargs)
    def assign(self, f, target=NAMED, **kwargs):
        plan = self.preview(f, target, **kwargs)
        assert plan['can_apply'], plan
        return self.rpc(f, 'set_repository_account', target=target, basis=plan['basis'], **kwargs)
    def github(self, f, action=None, repository=None, **kwargs):
        return self.rpc(f, 'github', request={'repository': repository, 'action': action or {'type': 'issues'}}, **kwargs)
    def record(self, f, config=None):
        path = self.registry if config is None else config
        return next(record for record in json.loads(path.read_text()).get('records', []) if record['path'] == path_json(f.path))
    def evidence(self, f, config=True):
        common = Path(os.fsdecode(f.git('rev-parse', '--path-format=absolute', '--git-common-dir').stdout.rstrip(b'\n')))
        index = Path(os.fsdecode(f.git('rev-parse', '--path-format=absolute', '--git-path', 'index').stdout.rstrip(b'\n')))
        return (f.git('for-each-ref', '--format=%(refname) %(objectname)').stdout,
                f.git('symbolic-ref', '-q', 'HEAD', check=False).stdout, index.read_bytes(),
                (common/'config').read_bytes() if config else None,
                f.git('config', 'user.name').stdout, f.git('config', 'user.email').stdout,
                (f.path/'file').read_bytes(), (f.path/'loose').read_bytes() if (f.path/'loose').exists() else None)
    def start_rpc(self, f, method, **params):
        child = subprocess.Popen([str(BINARY), '--config', str(self.registry), '-C', str(f.path), 'repo', '-'],
                                 env=self.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        child.stdin.write(json.dumps({'method': method, **params})); child.stdin.close(); child.stdin = None
        try: wait_for(self.started.exists, 'delayed gh request did not start'); return child
        except BaseException: stop(child); raise

    @contextmanager
    def server(self, f):
        child = subprocess.Popen([str(BINARY), '--config', str(self.registry), '-C', str(f.path), 'serve', '--port', '0'],
                                 env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        ws = None
        try:
            assert select.select([child.stderr], [], [], 12)[0], 'account server startup timed out'
            line = child.stderr.readline(); match = re.search(r'http://([^/]+)/#token=(\S+)', line)
            assert match, line
            authority, token = match.groups(); ws = WebSocket(authority, 'http://'+authority)
            ws.send({'token': token}); assert ws.receive()['ready']
            yield ws
        finally:
            if ws: ws.close()
            stop(child)


def call(ws, method, fail=False, **params):
    request_id = getattr(ws, 'account_request_id', 100)+1; ws.account_request_id = request_id
    ws.send({'id': request_id, 'method': method, **params})
    result = ws.receive(); clean(result)
    assert result['id'] == request_id, result
    if fail: assert 'error' in result, result; return result
    assert 'result' in result, result
    return result['result']
