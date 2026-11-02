import os
import sys

# The venv home and any cross-build override are set before import.
sys._home = sys.argv[2]
if sys._home == sys.argv[1] and "_PYTHON_PROJECT_BASE" in os.environ:
    del os.environ["_PYTHON_PROJECT_BASE"]
import sysconfig

assert sys.modules["sysconfig"] is sysconfig
assert sysconfig._PROJECT_BASE == sys.argv[1]
assert sysconfig._PYTHON_BUILD
assert sysconfig.is_python_build()

# The base is captured at import, but marker existence is checked on each call.
os.environ["_PYTHON_PROJECT_BASE"] = sys.argv[3]
assert sysconfig._PROJECT_BASE == sys.argv[1]
os.remove(os.path.join(sys.argv[1], "Modules", "Setup.local"))
assert not sysconfig.is_python_build(check_home=None)
assert sysconfig._PYTHON_BUILD
marker = os.path.join(sys.argv[1], "Modules", "Setup")
with open(marker, "wb") as stream:
    stream.write(b"")
assert sysconfig.is_python_build()

# A directory named Setup is not a build marker. Rebinding the module global
# must affect the next call even through a previously imported function.
from sysconfig import is_python_build
sysconfig._PROJECT_BASE = sys.argv[3]
assert not is_python_build()
sysconfig._PROJECT_BASE = sys.argv[1]
assert is_python_build()
assert sysconfig.get_path("stdlib") == sysconfig.get_paths()["stdlib"]
print("build markers passed")
