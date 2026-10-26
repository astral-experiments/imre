"""Run the original decoder key fixtures and distinguish native identities from repr transport."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bindings import Boundary, Probe
from reference import BENCH, COMMIT, routine, verify
from sweep import diagnostic, digest, prepare
sys.path.insert(0, str(BENCH))
from native_json_values import Native
from suite_inventory import cases, load_suite
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures

METHOD = "test.test_json.test_decode.TestDecode.test_keys_reuse"
SOURCE = BENCH / "upstream/Lib/test/test_json/test_decode.py"
OPERATIONS = "json.loads, native JSONDecoder construction and decode, sorted decoded keys, identity and memo truth"


def typed_body_fixture(*, string_identity=False, memo_truth=False, standalone=False):
    """Bind both public entry points without copying any decoded value."""
    prefix = """import json
import unittest
from typing import Any

class DecoderModule:
    def JSONDecoder(self):
        return json.decoder.JSONDecoder()

class JsonModule:
    decoder: DecoderModule
    def __init__(self):
        self.decoder = DecoderModule()

"""
    base = "" if standalone else "(unittest.TestCase)"
    init = "" if standalone else "        super().__init__()\n"
    prefix += f"""class NativeCase{base}:
    json: JsonModule
    def __init__(self):
{init}        self.json = JsonModule()

    def loads(self, source: str) -> list[dict[str, Any]]:
        return json.loads(source)

"""
    if string_identity:
        prefix += """    def assertIs(self, first: str, second: str) -> None:
        if first is not second:
            raise AssertionError("expected identical native key owners")

"""
    if memo_truth:
        prefix += """    def assertFalse(self, expr: dict[str, str], msg: str | None = None) -> None:
        if expr:
            raise AssertionError("expected an empty decoder memo")

"""
    return prefix + method_bytes(SOURCE, "TestDecode", "check_keys_reuse").decode()


def native_key_fixture():
    # A concrete child fixture can accept the two actual operand types without
    # narrowing TestCase's public assertion signatures. CPython owns lifecycle.
    return typed_body_fixture(string_identity=True, memo_truth=True, standalone=True)


def native_identity_control(tools, directory):
    """The same native assertion must reject equal keys from separate scans."""
    directory.mkdir()
    fixture = native_key_fixture()
    program = fixture + "\n" + method_bytes(SOURCE, "TestDecode", "test_keys_reuse").decode() + '''
case = NativeCase()
case.test_keys_reuse()
left = case.loads('[{"same_owner_spelling": 1}]')
right = case.loads(' [{"same_owner_spelling": 2}]')
left_keys, right_keys = sorted(left[0]), sorted(right[0])
short: list[str] = []
different = ["different spelling"]
if left_keys != right_keys or left_keys == short or left_keys == different:
    raise ValueError("expected string lists to compare by length and element value")
first, second = left_keys[0], right_keys[0]
if first != second or first is second:
    raise ValueError("expected equal keys from independent native scans")
try:
    case.assertIs(first, second)
except AssertionError:
    print("rejected distinct owners")
else:
    raise ValueError("the identity assertion accepted distinct native owners")
try:
    case.assertFalse({"nonempty": "memo"})
except AssertionError:
    print("rejected nonempty memo")
else:
    raise ValueError("the truth assertion accepted a nonempty mapping")
'''
    try:
        binary = tools.build(program.encode(), directory)
    except NativeCompileError as exc:
        return {"outcome": "blocked", "diagnostic": diagnostic(exc, directory)}
    child = subprocess.run([binary], capture_output=True, text=True, timeout=60)
    if child.returncode or child.stderr or child.stdout != "rejected distinct owners\nrejected nonempty memo\n":
        return {"outcome": "failed", "exit": child.returncode, "stdout": child.stdout, "stderr": child.stderr}
    return {"outcome": "pass", "operation": "distinct native string owners and nonempty memo rejected; string lists compared by value"}


def component_checks(compiler):
    """Isolate operations used by the unchanged helper; a check executes no method."""
    annotation = "from typing import Any, Callable\n"
    signature = "load: Callable[[str], list[dict[str, Any]]] = "
    programs = {
        "pass module loads": annotation + signature + "json.loads\nprint(repr(load('[]')))\n",
        "pass bound decoder decode": annotation + "decoder = json.JSONDecoder()\n" + signature
                                     + "decoder.decode\nprint(repr(load('[]')))\n",
        "sort decoded object": """rows = json.loads('[{"b_key": 1, "a_key": 2}]')
print(sorted(rows[0]))
""",
    }
    result = {}
    with tempfile.TemporaryDirectory() as work:
        entry = Path(work) / "case.py"
        for name, body in programs.items():
            source = "import json\n" + body
            entry.write_text(source)
            proc = subprocess.run([compiler.resolve(), "check", entry], capture_output=True, text=True, timeout=30)
            result[name] = {"source": source, "exit": proc.returncode,
                            "diagnostic": (proc.stderr + proc.stdout).strip().replace(str(entry), "<case>")}
    return result


# Emit the complete native result, followed by the native owner equivalence class
# of each key. Equal spellings must stay separate when the owners are distinct.
EXPOSE = b"""print(repr(doc))
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
DRIVER = b'import json\ndoc = json.loads(open("keys.txt").read())\n' + EXPOSE
DECODER = b'''import json
import sys
decoder = json.decoder.JSONDecoder()
print("READY")
sys.stdout.flush()
if input() != "decode":
    raise ValueError("expected decode")
doc = decoder.decode(open("keys.txt").read())
''' + EXPOSE + b'''print("END")
sys.stdout.flush()
if input() != "memo":
    raise ValueError("expected memo")
print(repr(decoder.memo))
'''


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


class DecoderSession:
    """Keep the actual native decoder alive until the original method reads its memo."""

    def __init__(self, program, directory, ops, traces):
        self.directory, self.ops, self.traces = directory, ops, traces
        self.child = subprocess.Popen([program], cwd=directory, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, bufsize=0)
        try:
            if self.receive(b"READY\n"):
                raise ValueError("unexpected constructor output")
            ops.append("json.decoder.JSONDecoder")
        except BaseException:
            self.close()
            raise

    def receive(self, marker):
        response = bytearray()
        while not response.endswith(marker):
            if not select.select([self.child.stdout], [], [], 30)[0]:
                raise TimeoutError("native decoder response")
            chunk = os.read(self.child.stdout.fileno(), 65536)
            if not chunk:
                raise ValueError(("incomplete native decoder response", self.child.stderr.read()))
            response.extend(chunk)
        return response[:-len(marker)].decode()

    def decode(self, text):
        (self.directory / "keys.txt").write_text(text)
        self.child.stdin.write(b"decode\n")
        value, owners = project(self.receive(b"END\n"))
        self.ops.append("JSONDecoder.decode")
        self.traces.append(owners)
        return value

    @property
    def memo(self):
        output, err = self.child.communicate(b"memo\n", timeout=30)
        if self.child.returncode or err:
            raise ValueError((self.child.returncode, output, err))
        self.ops.append("JSONDecoder.memo")
        return ast.literal_eval(output.decode())

    def close(self):
        if self.child.poll() is None:
            self.child.kill()
            self.child.communicate(timeout=30)


def owner_fixture(tools, work):
    """Bind the original operations to a decoder and return the observed key owners."""
    owner_directory = work / "owners"
    owner_directory.mkdir()
    owners = tools.build(DRIVER, owner_directory)
    decoder_directory = work / "decoder"
    decoder_directory.mkdir()
    decoder = tools.build(DECODER, decoder_directory)

    def execute(case, original):
        traces, ops, sessions = [], [], []

        def loads(document):
            (owner_directory / "keys.txt").write_text(document)
            output = subprocess.run([owners], cwd=owner_directory, capture_output=True, check=True, timeout=30)
            if output.stderr:
                raise ValueError(output.stderr)
            value, tokens = project(output.stdout.decode())
            traces.append(tokens)
            ops.append("json.loads")
            return value

        def constructor():
            session = DecoderSession(decoder, decoder_directory, ops, traces)
            sessions.append(session)
            return session

        case.json = SimpleNamespace(decoder=SimpleNamespace(JSONDecoder=constructor))
        case.loads = loads
        try:
            original()
        except AssertionError as exc:
            outcome, detail = "failed", str(exc)
        else:
            outcome, detail = "pass", None
        finally:
            for session in sessions:
                session.close()
        if outcome == "pass" and ops != ["json.loads", "json.decoder.JSONDecoder", "JSONDecoder.decode", "JSONDecoder.memo"]:
            raise ValueError(("missing native operation", ops))
        return {"outcome": outcome, "reason": detail, "repr_calls": [],
                "native_operations": ops, "native_key_owners": traces}
    return execute


def run(tools, work):
    plain_directory = work / "ordinary"
    plain_directory.mkdir()
    plain = tools.build((BENCH / "json_values_driver.py").read_bytes(), plain_directory)

    def ordinary(case, original):
        native = Native(plain, plain_directory, case.JSONDecodeError)
        case.json = Probe(native)
        case.loads = case.json.loads
        try:
            original()
        except (AssertionError, Boundary) as exc:
            outcome, reason = ("blocked" if isinstance(exc, Boundary) else "failed"), str(exc)
        else:
            outcome, reason = "pass", None
        return {"outcome": outcome, "reason": reason, "repr_calls": native.calls,
                "native_operations": [], "native_key_owners": []}

    ids = [case.id() for case in cases(load_suite("test.test_json")) if routine(case) == METHOD]
    if len(ids) != 2:
        raise ValueError(ids)
    results = {}
    for mode, execute in (("repr", ordinary), ("native-owners", owner_fixture(tools, work))):
        complete, failed, skipped = run_in_fixtures(load_suite("test.test_json"), dict.fromkeys(ids, execute))
        if failed or skipped or set(complete) != set(ids):
            raise ValueError((failed, skipped, complete))
        results[mode] = {name: records[0] for name, records in complete.items()}
    # Each complete-body attempt uses the original helper and test method.
    # An unshadowed fixture import isolates passing self.loads as a callable.
    owner, method = "TestDecode", "test_keys_reuse"
    general = prepare(SOURCE, owner, method)
    unshadowed = general.replace("import json\n", "import json as native_json\n")
    unshadowed = unshadowed.replace("    json = json\n", "    json = native_json\n")
    unshadowed = unshadowed.replace("staticmethod(json.loads)", "staticmethod(native_json.loads)")
    attempts = results["native-body-attempts"] = {}
    for name, fixture in (("general", general), ("unshadowed-module", unshadowed),
                          ("typed-module-bindings", typed_body_fixture()),
                          ("typed-string-identity", typed_body_fixture(string_identity=True)),
                          ("typed-key-assertions", typed_body_fixture(string_identity=True, memo_truth=True)),
                          ("native-key-assertions", native_key_fixture())):
        directory = work / name
        directory.mkdir()
        try:
            body = native_method(tools, directory, SOURCE, owner, ids[0], fixture, operation=("imre", OPERATIONS))
        except NativeCompileError as exc:
            attempts[name] = {"outcome": "blocked", "diagnostic": diagnostic(exc, directory)}
        else:
            passed, failed, skipped = run_in_fixtures(load_suite("test.test_json"), dict.fromkeys(ids, body))
            attempts[name] = {"passed": list(passed), "failed": failed, "skipped": skipped,
                              "observations": {key: [r["observation"] for r in rows] for key, rows in passed.items()}}
        attempts[name]["fixture_sha256"] = hashlib.sha256(fixture.encode()).hexdigest()
    results["native-distinct-owner-control"] = native_identity_control(tools, work / "distinct-owners")
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
              "helper_sha256": hashlib.sha256(method_bytes(SOURCE, "TestDecode", "check_keys_reuse")).hexdigest(),
              "sha256": {"imre": digest(args.imre), "runtime": digest(args.runtime)},
              "fixture": "CPython supplies both original module, class and instance lifecycles",
              "execution": {
                  "repr": "CPython body and assertions; native values transported as repr",
                  "native-owners": "CPython body and assertions; native key owners transported explicitly",
                  "native-body-attempts": "unchanged method and helper compiled together; individual outcomes below",
              },
              "operation": "Imre json.loads and a live JSONDecoder instance; key owner comparisons in Imre",
              "distinct-owner-negative-control": "passed",
              "compile-only-components": component_checks(args.imre), "levels": {}}
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as tmp:
            tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
            report["levels"][str(level)] = run(tools, Path(tmp))
    args.result.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
