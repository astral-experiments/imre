import os
import tempfile
from tempfile import TemporaryDirectory as Workspace

events = []


def argument(label, value):
    events.append(label)
    return value


owner = tempfile.TemporaryDirectory(
    argument("suffix", "-tail"),
    argument("prefix", "work-"),
    argument("dir", "scratch"),
    argument("ignore", True),
    delete=argument("delete", False),
)
assert events == ["suffix", "prefix", "dir", "ignore", "delete"]
assert type(owner) is Workspace
with owner as name:
    assert name == owner.name
    assert os.path.basename(name).startswith("work-") and name.endswith("-tail")
    os.symlink(os.path.abspath("outside"), os.path.join(name, "link"))
assert os.path.islink(os.path.join(name, "link"))
owner.cleanup()
owner.cleanup()
assert not os.path.exists(name) and os.path.isfile("outside/keep.txt")

for ignore in (False, True):
    try:
        with Workspace(dir="scratch", ignore_cleanup_errors=ignore) as root:
            os.rmdir(root)
            os.symlink(os.path.abspath("outside"), root)
    except OSError:
        assert not ignore
    else:
        assert ignore
    assert os.path.islink(root) and os.path.isfile("outside/keep.txt")
    os.unlink(root)
