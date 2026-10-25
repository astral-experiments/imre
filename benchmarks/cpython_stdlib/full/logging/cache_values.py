"""Observe cache aliases and manager suppression across successive native operations."""
import logging

parent = logging.getLogger("cache")
child = logging.getLogger("cache.child")
other = logging.getLogger("other")
root = logging.getLogger()
root.setLevel(-10)
saved = child._cache
print(saved is child._cache)
print(saved is other._cache)
print(child.isEnabledFor(0))
print(child.isEnabledFor(1))
print(saved)
logging.disable(-3)
print(saved)
print(child.isEnabledFor(-2))
print(other.isEnabledFor(-1))
print(child._cache)
print(other._cache)
parent.setLevel(15)
print(saved)
print(other._cache)
saved[1] = True
print(child.isEnabledFor(1))
logging.addLevelName(42, "CUSTOM")
logging.disable(level="CUSTOM")
print(saved)
print(child.isEnabledFor(42))
print(child.isEnabledFor(43))
try:
    logging.disable("unknown-level")
except ValueError as error:
    print(str(error))
print(saved)
try:
    logging.disable(None)
except TypeError as error:
    print(str(error))
try:
    parent.setLevel((0, 0))
except TypeError as error:
    print(str(error))
print(saved)
logging.disable(10**24)
print(child.isEnabledFor(10**24))
print(child.isEnabledFor(10**24 + 1))
logging.disable()
print(saved)
print(child.isEnabledFor(logging.CRITICAL))
logging.disable(False)
print(child.isEnabledFor(15))
