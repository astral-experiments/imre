import sys

old_base, old_exec_base = sys.base_prefix, sys.base_exec_prefix
sys.base_prefix, sys.base_exec_prefix = "/base/a/..", "/installed/exec/."
import sysconfig
sys.base_prefix, sys.base_exec_prefix = old_base, old_exec_base

assert sysconfig._CONFIG_VARS is None
try:
    sysconfig.get_config_vars(unsupported=True)
except TypeError:
    pass
else:
    raise AssertionError("keyword was accepted")
assert sysconfig._CONFIG_VARS is None
assert sysconfig.get_config_vars("installed_base", "installed_platbase") == ["/base", "/installed/exec"]

stdlib = sysconfig.get_path("stdlib")
assert stdlib == sysconfig.get_paths()["stdlib"]
if sys.implementation.name == "imre":
    version = sysconfig.get_config_var("py_version_short")
    assert stdlib == sys.base_prefix + "/lib/imre" + version
variables = sysconfig.get_config_vars()
assert sysconfig.get_config_vars() is variables
assert sysconfig.get_config_var(name="py_version_short") == variables["py_version_short"]
variables["imre_mutation_probe"] = 17
assert sysconfig.get_config_vars("imre_mutation_probe", "missing_imre_variable", "imre_mutation_probe") == [17, None, 17]
assert sysconfig.get_config_var("imre_mutation_probe") == 17
try:
    sysconfig.get_config_vars([])
except TypeError:
    pass
else:
    raise AssertionError("unhashable key was accepted")
old_prefix = sys.prefix
try:
    sys.prefix = "/some/prefix/."
    assert sysconfig.get_config_vars("prefix", "base") == ["/some/prefix", "/some/prefix"]
    assert sysconfig.get_config_var("imre_mutation_probe") is None
finally:
    sys.prefix = old_prefix
original = sysconfig.get_config_vars
try:
    sysconfig.get_config_vars = lambda: {"probe": 41}
    assert sysconfig.get_config_var("probe") == 41
finally:
    sysconfig.get_config_vars = original
print("stdlib found")

try:
    sysconfig.get_path("missing_path_name")
except KeyError:
    print("unknown path")
