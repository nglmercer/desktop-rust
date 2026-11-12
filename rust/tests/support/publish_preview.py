"""Shared publication request helpers for remaining legacy integration tests."""
from repository_accounts import NAMED


def request(target=NAMED, owner='alicecase', **overrides):
    return {'host':'github.com','target':target,'owner':owner,'name':'example-repo',
            'description':' exact description\n','private':True,**overrides}


def preview(s, f, value=None, **kwargs):
    return s.request(f,'publish_preview',request=value or request(),**kwargs)
