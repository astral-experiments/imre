"""Retain imported source modules for a loader call (startup witness #578)."""
import test as package
import test.dis_module as target
from test import dis_module as again


def module_name(value):
    return value.__name__


saved = target
print(saved is again)
print(saved == again)
print(saved is not package)
print(saved != package)
print(module_name(saved))
print(module_name(package))
print(saved.__file__.endswith("test/dis_module.py"))
for module in (saved, package, again):
    print(module.__name__)
