"""Read the version singleton in a different compiled module from its caller."""
from sys import version_info


def is_shared_version(value) -> bool:
    return value is version_info
