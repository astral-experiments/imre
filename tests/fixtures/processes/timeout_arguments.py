"""Timeout expression ordering and ownership use the same rooted call protocol."""

import subprocess
from sys import argv


def arguments(trace: list[str], owners: list[list[str]]) -> list[str]:
    trace.append("args")
    return owners[0]


def timeout(trace: list[str], owners: list[list[str]]) -> float | None:
    trace.append("timeout")
    owners.pop()
    return 5.0


def checking(trace: list[str]) -> bool:
    trace.append("check")
    return True


def fail_timeout(trace: list[str]) -> float:
    trace.append("failed timeout")
    raise ValueError("before launch")


trace: list[str] = []
owners = [[argv[1], "-I", "-c", "print('retained argv')"]]
result = subprocess.run(args=arguments(trace, owners), check=checking(trace), timeout=timeout(trace, owners), capture_output=True, encoding="utf-8")
print(repr(trace))
print(result.stdout)
print(len(owners))

owners = [[argv[1], "-I", "-c", "print('unreachable child')"]]
try:
    subprocess.run(timeout=fail_timeout(trace), args=arguments(trace, owners), capture_output=True, encoding="utf-8")
except ValueError as error:
    print(str(error))
print(repr(trace))
print(len(owners))
