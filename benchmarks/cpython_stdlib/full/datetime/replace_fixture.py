"""Run the unchanged CPython replacement test over a native datetime worker."""
import ast
from collections import Counter
from pathlib import Path
import subprocess
from unittest.mock import patch

from suite_evidence import roles


def prepare(tools, directory, method_sha256):
    binary = tools.build(Path(__file__).with_name("replace_worker.py").read_bytes(), directory)

    def execute(case, original):
        base = case.theclass
        requests = Counter()

        class NativeReplace(base):
            def replace(self, **changes):
                if self.tzinfo is not None or len(changes) > 1:
                    raise AssertionError("unexpected upstream replacement shape")
                name, value = next(iter(changes.items()), ("", 0))
                requests[name] += 1
                fields = (self.year, self.month, self.day, self.hour, self.minute,
                          self.second, self.microsecond, self.fold, name, value)
                result = subprocess.run([binary, *map(str, fields)], capture_output=True, check=True, timeout=15)
                assert not result.stderr, result.stderr
                values = ast.literal_eval(result.stdout.decode())
                if values[0] == "ValueError":
                    raise ValueError(values[1])
                return base(*values[:7], fold=values[7])

            __replace__ = replace

        with patch.object(case, "theclass", NativeReplace):
            original()
        assert requests == {"": 2, "year": 4, "month": 2, "day": 2, "hour": 2,
                            "minute": 2, "second": 2, "microsecond": 2}, requests
        return {
            "roles": roles(body="cpython", assertions="cpython", operation="mixed", fixture="mixed"),
            "evidence": {
                "method_sha256": method_sha256,
                "opt_level": tools.level,
                "operation": "Imre constructs the native source and executes all 18 replacements, including two ValueErrors. "
                             "CPython dispatches copy.replace and compares the returned civil fields.",
                "fixture": "CPython's unchanged Pure/Fast TestCase uses a datetime subclass as a transport to the native worker.",
            },
        }

    return execute
