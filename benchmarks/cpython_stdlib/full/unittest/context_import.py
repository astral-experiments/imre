"""Identity and payload of ImportError and ModuleNotFoundError inside assertion scopes."""
import unittest

case = unittest.TestCase()
missing = ModuleNotFoundError("missing", name="pkg.child", path="pkg.py")
with case.assertRaises(ImportError) as parent:
    with case.assertRaises(LookupError):
        raise missing
with case.assertRaises(ModuleNotFoundError) as child:
    raise missing
print(parent.exception is missing)
print(child.exception is missing)
print(missing.name)
print(missing.path)
original = ImportError("guard")
with case.assertRaises(ImportError) as guard:
    with case.assertRaises(ModuleNotFoundError):
        raise original
print(guard.exception is original)
