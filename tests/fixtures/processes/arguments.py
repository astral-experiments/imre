import os
import subprocess
from pathlib import Path
from sys import argv


def environment(trace: list[str], values: dict[str, str]) -> dict[str, str]:
    trace.append("env")
    return values


def arguments(trace: list[str], values: dict[str, str], command: list[str]) -> list[str]:
    trace.append("args")
    values["RELAR_PROCESS_OVERRIDE"] = "changed during args"
    return command


def directory(trace: list[str]) -> Path:
    trace.append("cwd")
    return Path("directory")


def checking(trace: list[str], command: list[str]) -> bool:
    trace.append("check")
    command.append("late argument")
    return True


def release(owners: list[list[str]]) -> dict[str, str]:
    owners.pop()
    return {}


def fail() -> bool:
    print("last argument failed")
    raise ValueError("before launch")


trace: list[str] = []
values = os.environ.copy()
command = [argv[1], "-I", argv[2], "environment"]
result = subprocess.run(env=environment(trace, values), args=arguments(trace, values, command), cwd=directory(trace), check=checking(trace, command), capture_output=True, encoding="utf-8")
print(repr(trace))
print(result.args[-1])
print(result.stdout)
echo_command = [argv[1], "-I", argv[2], "echo", "initial argument"]
echo = subprocess.run(echo_command, capture_output=True, encoding="utf-8", check=checking(trace, echo_command))
print(echo.stdout)

owners = [[argv[1], "-I", argv[2], "echo", "only temporary owner remains"]]
print(subprocess.run(args=owners[0], env=release(owners), capture_output=True, encoding="utf-8").stdout)
print(len(owners))
try:
    subprocess.run([argv[1], "-I", argv[2], "write", "forbidden.txt"], capture_output=True, encoding="utf-8", check=fail())
except ValueError as error:
    print(str(error))
print(Path("forbidden.txt").exists())
