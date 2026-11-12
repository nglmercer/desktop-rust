"""Shared LFS helpers for the remaining Python integration suites."""
import hashlib
import os
from pathlib import Path


def pointer(body):
    return f"version https://git-lfs.github.com/spec/v1\noid sha256:{hashlib.sha256(body).hexdigest()}\nsize {len(body)}\n".encode()


def cached(f, body):
    output = f.git('lfs', 'env').stdout
    media = Path(os.fsdecode(next(line.removeprefix(b'LocalMediaDir=') for line in output.splitlines() if line.startswith(b'LocalMediaDir='))))
    oid = hashlib.sha256(body).hexdigest()
    return media / oid[:2] / oid[2:4] / oid
