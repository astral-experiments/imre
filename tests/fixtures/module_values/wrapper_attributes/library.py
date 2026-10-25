import functools as tools
from functools import WRAPPER_ASSIGNMENTS as assigned


def assignments(fields=assigned) -> tuple[str, str, str, str, str, str]:
    return fields


def uses_default(fields=tools.WRAPPER_UPDATES) -> bool:
    return fields is tools.WRAPPER_UPDATES


def single(name: str) -> tuple[str]:
    return (name,)
