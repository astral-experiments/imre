import sys
import sysconfig

paths = sys.path
paths.append("imre-search-entry")
print(sys.path[-1])
print(paths is sys.path)

stdlib = sysconfig.get_path("stdlib")
print(isinstance(stdlib, str))
print(bool(stdlib))
