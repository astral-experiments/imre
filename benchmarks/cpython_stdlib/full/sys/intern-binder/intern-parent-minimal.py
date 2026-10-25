import sys
from importlib.machinery import ModuleSpec

spec = ModuleSpec("pkg.mod", None)
value = spec.parent
print(sys.intern(value))
