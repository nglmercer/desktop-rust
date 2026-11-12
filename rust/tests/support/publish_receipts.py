"""Shared publication receipt helpers for remaining integration tests."""
import hashlib
import json
import os
import subprocess
import uuid

from support.fixture import BINARY
from repository_context import private


def digest(value):
    return hashlib.sha256(value).hexdigest()


def receipt_dir(s,f,registry=None):
    registry=s.registry if registry is None else registry
    record=next(row for row in json.loads(registry.read_text())['records'] if row['path']==str(f.path))
    namespace=digest(os.fsencode(registry.absolute()))
    repository_id=digest(os.fsencode(f.path))
    record_id=record['id']
    directory=digest(json.dumps([namespace,record_id],separators=(',',':')).encode())
    return (f.path/'.git/desktop-publish-receipts'/directory, namespace, repository_id, record_id)


def synthetic(s,f,phase='created',error=None,registry=None):
    directory,namespace,repository_id,record_id=receipt_dir(s,f,registry)
    identifier=str(uuid.uuid4())
    value={'version':1,'id':identifier,'repository_id':repository_id,'namespace':namespace,
           'record_id':record_id,'phase':phase,'host':'github.com','owner':'ExampleOrg',
           'name':'example-repo','error':error,'acknowledged':False}
    directory.mkdir(parents=True,exist_ok=True,mode=0o700)
    path=directory/(identifier+'.json'); private(path,json.dumps(value))
    return path,value


def rpc(s,f,method,*,config=None,fail=False,timeout=15,**params):
    command=[str(BINARY),'--config',str(s.registry if config is None else config),'-C',str(f.path),'repo','-']
    result=subprocess.run(command,input=json.dumps({'method':method,**params}),env=s.env,
                          capture_output=True,text=True,timeout=timeout)
    s.assert_clean(result.stdout+result.stderr)
    if fail:
        assert result.returncode!=0,(method,result.stdout,result.stderr)
        return result
    assert result.returncode==0,(method,result.stderr,result.stdout)
    return json.loads(result.stdout)


def history(s,f,**kwargs): return rpc(s,f,'publish_history',**kwargs)
def acknowledge(s,f,identifier,**kwargs): return rpc(s,f,'acknowledge_publish',receipt_id=identifier,**kwargs)
