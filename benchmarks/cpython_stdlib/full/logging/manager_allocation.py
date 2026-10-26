"""Run with manager_allocation.c to reject the first allocation in each path."""
import logging


def shared(logger: logging.Logger) -> logging.Manager:
    return logger.manager


def private(logger: logging.Logger) -> logging.Manager:
    return logging.Manager(logger)


logger = logging.getLogger("allocation")
try:
    shared(logger)
    raise AssertionError("expected the shared allocation to fail")
except MemoryError:
    print("caught shared MemoryError")

manager = shared(logger)
assert manager is logging.getLogger().manager
assert manager.root is logging.getLogger()

try:
    private(logger)
    raise AssertionError("expected the private allocation to fail")
except MemoryError:
    print("caught private MemoryError")

other = private(logger)
assert other is not manager
assert other.root is logger
assert logging.Manager(None).root is None
print("manager identities and roots survive retry")
