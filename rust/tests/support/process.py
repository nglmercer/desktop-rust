"""Process helpers retained by Python suites that have not yet been migrated."""
import os
from pathlib import Path


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    # A killed child can await OS reaping after its owner exits.
    status = Path(f'/proc/{pid}/status')
    try:
        if status.exists() and 'State:\tZ' in status.read_text():
            return False
    except (FileNotFoundError, ProcessLookupError):
        return False
    return True
