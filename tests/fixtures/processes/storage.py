from dataclasses import dataclass
from subprocess import CompletedProcess, run
from sys import argv


class Holder:
    process: CompletedProcess[str]

    def __init__(self, process: CompletedProcess[str]) -> None:
        self.process = process


@dataclass(slots=True)
class Envelope:
    process: CompletedProcess[str]


def invoke(command: list[str]) -> CompletedProcess:
    return run(command, capture_output=True, encoding="utf-8")


def retained(values: list[CompletedProcess[str]]) -> CompletedProcess[str]:
    return values.pop()


command = [argv[1], "-I", argv[2], "echo", "stored result"]
first = invoke(command)
second = invoke(command)
holder = Holder(first)
values = [first, second]
mapping = {"first": first, "second": second}
optional: CompletedProcess[str] | None = mapping.get("first")
print(f"{first == first} {first == second} {first != second}")
print(f"{isinstance(first, CompletedProcess)} {isinstance(0, CompletedProcess)}")
print(repr(first))
print(str(first) == repr(first))
print(f"{first}")
print(f"{holder.process == first} {values[0] == mapping['first']}")
envelope = Envelope(first)
print(f"{envelope == Envelope(first)} {envelope == Envelope(second)}")
print(repr(envelope))
if optional is not None:
    print(f"{optional.returncode} {optional.stdout}")
    optional.args.append("through optional")
print(command[-1])
holder.process = second
print(f"{holder.process == first} {holder.process == second}")
print(retained(values).stdout)
print(len(values))
optional = None
print(optional is None)
maybe_command: list[str] | None = command
if maybe_command is not None:
    print(invoke(maybe_command).stdout)
print(invoke([argv[1], "-I", argv[2], "empty"]).check_returncode())
