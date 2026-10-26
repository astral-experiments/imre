"""Marshal one FileStorage path operand into a typed native call."""
import ast
from pathlib import Path
import subprocess

from suite_native import NativeCompileError


class MyStr(str):
    pass


class PathValue:
    def __init__(self, value):
        self.value = value
        self.calls = 0

    def __fspath__(self):
        self.calls += 1
        return self.value


class Refused:
    def __init__(self):
        self.calls = 0

    def __fspath__(self):
        self.calls += 1
        raise OSError("path refused")


def worker_source(operation, value_type, owner):
    """A refusal to compile a str subclass must only block cases with that operand."""
    source = f"from os import {operation}\nfrom pathlib import Path\n"
    if value_type == "MyStr":
        source += "class MyStr(str):\n    pass\n"
    if owner is PathValue:
        source += f"""class PathValue:
    value: {value_type}
    calls: int
    def __init__(self, value: {value_type}) -> None:
        self.value = value
        self.calls = 0
    def __fspath__(self) -> {value_type}:
        self.calls += 1
        return self.value
"""
    if owner is Refused:
        source += """class Refused:
    calls: int
    def __init__(self) -> None:
        self.calls = 0
    def __fspath__(self) -> str:
        self.calls += 1
        raise OSError("path refused")
arg = Refused()
"""
    else:
        text = 'open("input", encoding="utf-8").read()'
        read = {"str": text, "MyStr": f"MyStr({text})", "Path": f"Path({text})",
                "bytes": 'open("input", "rb").read()', "int": "17"}[value_type]
        source += f"value = {read}\narg = " + ("PathValue(value)" if owner is PathValue else "value") + "\n"
    compare = (value_type in {"str", "MyStr"} or (value_type == "bytes" and operation == "fspath"))
    source += f"""try:
    result = {operation}(arg)
    print("ok")
    print(repr(result))
    print({"result is value" if compare else "False"})
"""
    for name in ("TypeError", "OSError", "NotImplementedError"):
        source += f'except {name} as error:\n    print("{name}")\n    print(repr(str(error)))\n    print(False)\n'
    source += "print(arg.calls)\n" if owner in {PathValue, Refused} else "print(-1)\n"
    return source.encode()


class NativePaths:
    def __init__(self, tools, directory):
        self.tools = tools
        self.directory = directory
        self.workers = {}

    def __call__(self, operation, operand, event):
        owner = type(operand)
        value = operand.value if owner is PathValue else operand
        value_type = "Path" if isinstance(value, Path) else type(value).__name__
        key = operation, value_type, owner.__name__
        directory = self.directory / "-".join(key)
        if key not in self.workers:
            directory.mkdir()
            try:
                self.workers[key] = self.tools.build(worker_source(operation, value_type, owner), directory)
            except NativeCompileError as exc:
                self.workers[key] = NativeCompileError(str(exc).splitlines()[-1].split("Imre: ")[-1])
        binary = self.workers[key]
        if isinstance(binary, NativeCompileError):
            raise binary
        if isinstance(value, bytes):
            (directory / "input").write_bytes(value)
        elif isinstance(value, (str, Path)):
            (directory / "input").write_text(str(value), encoding="utf-8")
        run = subprocess.run([binary], cwd=directory, capture_output=True, text=True, check=True, timeout=30)
        assert not run.stderr, run.stderr
        status, literal, identical, calls = run.stdout.splitlines()
        event["executed_in_imre"] = True
        if int(calls) >= 0:
            event["descriptor_calls"] = int(calls)
        result = ast.literal_eval(literal)
        if status != "ok":
            raise {"TypeError": TypeError, "OSError": OSError, "NotImplementedError": NotImplementedError}[status](result)
        if identical == "True":
            assert result == value and isinstance(value, (str, bytes))
            return value
        return result
