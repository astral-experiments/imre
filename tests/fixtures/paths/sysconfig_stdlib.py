import sys
import sysconfig

stdlib = sysconfig.get_path("stdlib")
assert stdlib == sysconfig.get_paths()["stdlib"]
if sys.implementation.name == "imre":
    version = sysconfig.get_config_var("py_version_short")
    assert stdlib == sys.base_prefix + "/lib/imre" + version
print("stdlib found")

try:
    sysconfig.get_path("missing_path_name")
except KeyError:
    print("unknown path")
