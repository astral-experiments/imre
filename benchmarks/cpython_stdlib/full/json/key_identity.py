"""Run the original decoder key fixtures and distinguish native identities from repr transport."""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bindings import Boundary, Probe
from reference import BENCH, COMMIT, routine, verify
sys.path.insert(0, str(BENCH))
from native_json_values import Native
from suite_inventory import cases, load_suite
from suite_native import Toolchain, method_bytes, run_in_fixtures

METHOD = "test.test_json.test_decode.TestDecode.test_keys_reuse"
SOURCE = BENCH / "upstream/Lib/test/test_json/test_decode.py"
# Emit the complete native result, followed by the native owner equivalence class
# of each key. Equal spellings must stay separate when the owners are distinct.
DRIVER = b"""import json
doc = json.loads(open("keys.txt").read())
print(repr(doc))
seen: list[str] = []
for row in doc:
    for key in row:
        name: str = key
        owner = 0
        while owner < len(seen) and seen[owner] is not name:
            owner += 1
        if owner == len(seen):
            seen.append(name)
        print(owner)
"""


class ProjectedKey(str):
    """A freshly allocated host string for one explicitly observed native owner."""


def project(data):
    payload, *tokens = data.splitlines()
    result = ast.literal_eval(payload)
    names, owners = iter(tokens), {}
    for index, row in enumerate(result):
        converted = {}
        for text, value in row.items():
            owner = int(next(names))
            key = owners.setdefault(owner, ProjectedKey(text))
            if key != text:
                raise ValueError("one native owner supplied two spellings")
            converted[key] = value
        result[index] = converted
    if next(names, None) is not None:
        raise ValueError("extra native key owner")
    return result, [int(t) for t in tokens]


def run(tools, work):
    plain_directory = work / "ordinary"
    plain_directory.mkdir()
    plain = tools.build((BENCH / "json_values_driver.py").read_bytes(), plain_directory)
    owner_directory = work / "owners"
    owner_directory.mkdir()
    owners = tools.build(DRIVER, owner_directory)
    ids = [case.id() for case in cases(load_suite("test.test_json")) if routine(case) == METHOD]
    if len(ids) != 2:
        raise ValueError(ids)
    results = {}
    for mode in ("repr", "native-owners"):
        def execute(case, original):
            native = Native(plain, plain_directory, case.JSONDecodeError)
            proxy = Probe(native)
            traces = []

            def loads(document):
                (owner_directory / "keys.txt").write_text(document)
                output = subprocess.run([owners], cwd=owner_directory, capture_output=True, check=True)
                if output.stderr:
                    raise ValueError(output.stderr)
                value, tokens = project(output.stdout.decode())
                traces.append(tokens)
                return value

            case.json = proxy
            case.loads = proxy.loads if mode == "repr" else loads
            try:
                original()
            except (AssertionError, Boundary) as exc:
                outcome = "blocked" if isinstance(exc, Boundary) else "failed"
                detail = str(exc)
            else:
                outcome, detail = "pass", None
            return {"outcome": outcome, "reason": detail, "repr_calls": native.calls, "native_key_owners": traces}

        complete, failed, skipped = run_in_fixtures(load_suite("test.test_json"), dict.fromkeys(ids, execute))
        if failed or skipped or set(complete) != set(ids):
            raise ValueError((failed, skipped, complete))
        results[mode] = {name: records[0] for name, records in complete.items()}
    return results


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    args = p.parse_args()
    verify()
    # Deliberately distinct native tokens with the same spelling must not pass
    # the upstream identity assertion merely because both spellings are equal.
    values, _ = project("[{'repeated_key': 1}, {'repeated_key': 2}]\n0\n1\n")
    a, b = list(values[0])[0], list(values[1])[0]
    if a != b or a is b:
        raise ValueError("host projection conflated distinct native owners")
    report = {"cpython_commit": COMMIT, "method": METHOD,
              "method_sha256": hashlib.sha256(method_bytes(SOURCE, "TestDecode", "test_keys_reuse")).hexdigest(),
              "fixture": "CPython module, class, instance, unmodified method, sorted and identity assertions",
              "operation": "one Imre json.loads; key owner comparisons in Imre",
              "distinct-owner-negative-control": "passed", "levels": {}}
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as tmp:
            tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
            report["levels"][str(level)] = run(tools, Path(tmp))
    args.result.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
