"""Continuation setup used by remaining publication-origin tests."""
import copy

from publish_create import create, setup


def prepare(s,name,*,org=False):
    f,review,owner=setup(s,name,org=org)
    result=create(s,f,review,owner=owner)
    assert result['phase']=='created'
    response=copy.deepcopy(s.data['post_response'])
    endpoint='/repos/'+owner.lower()+'/example-repo'
    s.update(repository_responses={endpoint:response})
    s.clear_calls()
    return f,result['receipt_id'],owner,endpoint,response
