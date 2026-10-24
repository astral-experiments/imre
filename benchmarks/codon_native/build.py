"""Prepare explicit native Codon adapters and build the shared tool algorithms."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
COMPILER_SHA256 = "cde8229e41a67e87edce088fbbd851acdf1b62e913677bb7cd9a9ce5d61c5350"
LIBRARIES = {
    "libcodonc.so": "8aeedf95ba48fbcb7be24b9acb2e213a86b7773c46c40d05eee54c2526edc4b0",
    "libcodonrt.so": "b7dbed21f9bd086d6331581b0bc330e48abab14766e3a69fb970c9855baf878d",
}
FILES = {
    "conformance_report": ["__init__.py", "__main__.py", "compute.py", "app.py", "model.py", "parse.py", "render.py", "live.py"],
    "python_catalog": ["__init__.py", "__main__.py", "compute.py", "catalog.py"],
}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def bodies(source: str, *, restore_calls: bool = False) -> dict[str, str]:
    """Bind application function bodies independently of declaration adapters."""
    result = {}

    def visit(nodes, prefix):
        for node in nodes:
            if isinstance(node, (ast.ClassDef, ast.FunctionDef)):
                name = prefix + node.name
                if isinstance(node, ast.FunctionDef):
                    body = ast.dump(ast.Module(body=node.body, type_ignores=[]), include_attributes=False)
                    result[name] = digest(body.encode())
                visit(node.body, name + ".")

    tree = ast.parse(source)
    if restore_calls:
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
                if node.func.id == "native_python_int":
                    node.func.id = "int"
                elif node.func.id == "native_python_sorted":
                    node.func.id = "sorted"
    visit(tree.body, "")
    return result


def adapt(source: str, name: str) -> tuple[str, list[dict]]:
    """Apply declared boundaries, checking all remaining function-body AST nodes."""
    edits = []
    body_edits = []
    lines = source.splitlines(keepends=True)
    tree = ast.parse(source)
    calls = [node for node in ast.walk(tree)
             if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
             and (node.func.id == "int" or node.func.id == "sorted" and any(kw.arg == "key" for kw in node.keywords))]
    if any(isinstance(node, ast.Name) and node.id in ("native_python_int", "native_python_sorted") for node in ast.walk(tree)):
        raise ValueError("native boundary name conflicts with application")
    for node in sorted(calls, key=lambda node: (node.lineno, node.col_offset), reverse=True):
        original = node.func.id
        allowed_keywords = set() if original == "int" else {"key", "reverse"}
        if len(node.args) != 1 or any(kw.arg not in allowed_keywords for kw in node.keywords):
            raise ValueError("unsupported native boundary signature")
        replacement = "native_python_int" if original == "int" else "native_python_sorted"
        line = lines[node.lineno - 1].encode()
        start, end = node.func.col_offset, node.func.end_col_offset
        if line[start:end] != original.encode():
            raise ValueError("unexpected native boundary spelling")
        lines[node.lineno - 1] = (line[:start] + replacement.encode() + line[end:]).decode()
        edits.append({"kind": "native Unicode decimal conversion" if original == "int" else "native eager-key sorting", "line": node.lineno,
                      "column": start, "before": ast.get_source_segment(source, node),
                      "callee_before": original, "callee_after": replacement})
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.args.kwonlyargs and node.args.vararg is None:
            start, stop = node.lineno - 1, node.body[0].lineno - 1
            before = "".join(lines[start:stop])
            if before.count("*,") != 1:
                raise ValueError("expected one keyword-only separator")
            after = before.replace("*, ", "").replace("*,\n", "\n")
            if name == "conformance_report/render.py" and node.name == "render_test_cases":
                after = after.replace('Literal["diff", "github"]', "str")
            lines[start:stop] = after.splitlines(keepends=True)
            edits.append({"kind": "native function declaration", "line": node.lineno, "before": before, "after": after})
        if not isinstance(node, ast.ClassDef):
            continue
        if name == "conformance_report/model.py" and node.name in {
            "Position", "Positions", "Location", "TyDiagnostic", "ExpectedError", "TestCase",
        }:
            # Codon dataclasses use tuple-style display. Ruff shows the Python record on assertion failure.
            fields = [item.target.id for item in node.body if isinstance(item, ast.AnnAssign)]
            pieces = [f"{field}={{self.{field}!r}}" for field in fields]
            if node.name == "Location":
                pieces[fields.index("path")] = "path=PosixPath({self.path.as_posix()!r})"
            display = node.name + "(" + ", ".join(pieces) + ")"
            lines[node.end_lineno - 1] += f'\n    def __repr__(self) -> str:\n        return f"{display}"\n'
            edits.append({"kind": "native Python dataclass repr", "class": node.name, "fields": fields})
        for decorator in node.decorator_list:
            if isinstance(decorator, ast.Call) and isinstance(decorator.func, ast.Name) and decorator.func.id == "dataclass":
                if decorator.lineno != decorator.end_lineno or decorator.args:
                    raise ValueError("expected one-line keyword-only dataclass declaration")
                options = {keyword.arg: ast.literal_eval(keyword.value) for keyword in decorator.keywords}
                if not options.keys() <= {"slots", "kw_only", "frozen", "eq"}:
                    raise ValueError("unreviewed dataclass options")
                before = lines[decorator.lineno - 1]
                after = "@dataclass(eq=" + str(options.get("eq", True)) + ")\n"
                lines[decorator.lineno - 1] = after
                edits.append({"kind": "native dataclass declaration", "line": decorator.lineno, "before": before, "after": after})
    # Codon's native Enum has singleton members, value patterns and string
    # payloads. Spell out StrEnum's auto values and Python string display.
    for node in tree.body:
        if not (isinstance(node, ast.ClassDef) and any(isinstance(base, ast.Name) and base.id == "StrEnum" for base in node.bases)):
            continue
        members = [item for item in node.body if isinstance(item, ast.Assign)]
        if not members:
            raise ValueError("empty conformance StrEnum")
        for member in members:
            if not (len(member.targets) == 1 and isinstance(member.targets[0], ast.Name)
                    and isinstance(member.value, ast.Call) and isinstance(member.value.func, ast.Name)
                    and member.value.func.id == "auto" and not member.value.args and not member.value.keywords):
                raise ValueError("unexpected conformance StrEnum member")
            key = member.targets[0].id
            lines[member.lineno - 1] = f"    {key} = {key.lower()!r}\n"
        lines[node.lineno - 1] = f"class {node.name}(Enum):\n"
        lines[members[-1].end_lineno - 1] += "\n    def __str__(self) -> str:\n        return self.value\n"
        edits.append({"kind": "native singleton Enum with StrEnum values and display", "class": node.name,
                      "members": [m.targets[0].id for m in members]})
    changed = "".join(lines)
    if name in ("conformance_report/model.py", "conformance_report/parse.py") and "from collections.abc import Sequence\n" in changed:
        changed = changed.replace("from collections.abc import Sequence\n", "")
        before, after = "test_files: Sequence[Path]", "test_files: list[Path]"
        if changed.count(before) != 1:
            raise ValueError("unexpected conformance sequence annotation")
        changed = changed.replace(before, after)
        edits.append({"kind": "native concrete sequence declaration", "before": before, "after": after})
    if name == "conformance_report/model.py" and "from collections import defaultdict\n" in changed:
        for value in ("TyDiagnostic", "ExpectedError"):
            before = f": defaultdict[str, list[{value}]] = defaultdict(list)"
            after = f" = defaultdict(list[{value}])"
            body_edits.append((before, after))
        for mapping in ("old_by_key", "new_by_key", "expected_by_key"):
            keys = f"{mapping}.keys()"
            body_edits.append((keys, f"set({keys})"))
        body_edits.append((
            "(e.location.path.name, e.location.positions.begin.line): e.tag",
            "(e.location.path.name, e.location.positions.begin.line): str(e.tag)",
        ))
        for before, after in body_edits:
            if before not in changed:
                raise ValueError(f"unexpected grouping boundary: {before}")
            changed = changed.replace(before, after)
            edits.append({"kind": "native grouping default factory or dictionary keys", "before": before, "after": after})
    if any(node.func.id == "int" for node in calls):
        changed = "from codon_decimal import parse_int as native_python_int\n\n" + changed
    if any(node.func.id == "sorted" for node in calls):
        changed = "from codon_sort import sort_with_key as native_python_sorted\n\n" + changed
    if "from enum import StrEnum, auto\n" in changed:
        changed = changed.replace("from enum import StrEnum, auto\n", "from enum import Enum\n")
        changed = changed.replace("from typing import Literal\n", "")
        literal = 'Literal["added", "removed", "changed"]'
        if changed.count(literal) != 1:
            raise ValueError("unexpected conformance title annotation")
        changed = changed.replace(literal, "str")
        edits.append({"kind": "closed title literal annotation", "before": literal, "after": "str"})
    if name == "conformance_report/render.py" and "from itertools import groupby\n" in changed:
        changed = changed.replace("from typing import Literal\n", "")
        # Codon passes the two module-level URL bindings through the nested row renderer.
        for before, after, count in (
            ("def render_html_diff_row(tc: TestCase, source:", "def render_html_diff_row(tc: TestCase, CONFORMANCE_URL: str, CONFORMANCE_DIR_WITH_README: str, source:", 1),
            ("lines.extend(render_html_diff_row(tc, source=", "lines.extend(render_html_diff_row(tc, CONFORMANCE_URL, CONFORMANCE_DIR_WITH_README, source=", 2),
        ):
            if changed.count(before) != count:
                raise ValueError("unexpected diagnostic row signature or calls")
            changed = changed.replace(before, after)
            body_edits.append((before, after))
            edits.append({"kind": "pass checkout URLs directly to the HTML row helper", "before": before, "after": after})
        before = "                url = CONFORMANCE_URL.format(filename=filename, line=min_line)"
        after = """                fields: dict[str, str | int] = {}
                fields["filename"] = filename
                fields["line"] = min_line
                url = CONFORMANCE_URL.format_map(fields)"""
        if changed.count(before) != 1:
            raise ValueError("unexpected diagnostic URL template")
        changed = changed.replace(before, after)
        body_edits.append((before, after))
        edits.append({"kind": "native named formatting with format_map", "before": before, "after": after})
    if name == "conformance_report/render.py" and "from textwrap import dedent\n" in changed:
        # Codon appends its source location to messages raised by assert statements.
        before = "        assert new.true_positives > 0, ("
        after = "        if not (new.true_positives > 0):\n            raise AssertionError("
        if changed.count(before) != 1:
            raise ValueError("unexpected summary assertion")
        changed = changed.replace(before, after)
        body_edits.append((before, after))
        edits.append({"kind": "preserve the summary assertion payload", "before": before, "after": after})
    if name == "conformance_report/render.py" and "from typing import assert_never\n" in changed:
        changed = changed.replace("from typing import assert_never\n",
                                  'def assert_never(value):\n    raise AssertionError(f"Expected code to be unreachable, but got: {value!r}")\n')
        edits.append({"kind": "native assert_never for exhaustive Change branches"})
    dataclass_import = "from dataclasses import dataclass\n"
    if dataclass_import in changed:
        if changed.count(dataclass_import) != 1:
            raise ValueError("unexpected dataclass import")
        changed = changed.replace(dataclass_import, "")
        edits.append({"kind": "native builtin dataclass import", "before": dataclass_import, "after": ""})
    if name == "conformance_report/parse.py" and "import re\n" in changed:
        changed = changed.replace("import re\n", "import codon_conformance_re as re\n")
        edits.append({"kind": "native compiled verbose regex and Python whitespace boundary",
                      "adapter": "conformance_re.py"})
    if name in ("conformance_report/parse.py", "python_catalog/catalog.py"):
        before = "from relar.json import decode\n"
        if changed.count(before) != 1:
            raise ValueError("typed decoder import changed")
        changed = changed.replace(before, "import json\n")
        adapter = "conformance_json.py" if name.startswith("conformance_report/") else "catalog_json.py"
        addition = (HERE / adapter).read_text()
        changed += "\n\n" + addition
        edits.append({"kind": "native typed JSON boundary", "adapter": adapter, "adapter_sha256": digest(addition.encode())})
    if name in ("python_catalog/__main__.py", "python_catalog/compute.py"):
        before = "from relar.json import encode\n"
        if changed.count(before) != 1:
            raise ValueError("typed encoder import changed")
        changed = changed.replace(before, "from .catalog import encode\n")
        edits.append({"kind": "native encoder import"})
    if name == "python_catalog/__main__.py":
        # Codon specializes this function for its native File[str] and gzFile[str].
        declaration = "def read_catalog(stream: TextIO)"
        imported = "from typing import TextIO\n"
        if changed.count(declaration) != 1 or changed.count(imported) != 1:
            raise ValueError("catalog text reader declaration changed")
        changed = changed.replace(imported, "").replace(declaration, "def read_catalog(stream)")
        edits.append({"kind": "specialize native plain and gzip text handles", "before": declaration,
                      "after": "def read_catalog(stream)"})
    if name == "conformance_report/live.py":
        before = "from relar.json import decode\n"
        if changed.count(before) != 1:
            raise ValueError("live typed JSON import changed")
        changed = changed.replace(before, "from conformance_report.parse import decode\n")
        before = "from relar.toml import decode as decode_toml\n"
        if changed.count(before) != 1:
            raise ValueError("live typed TOML import changed")
        changed = changed.replace(before, "")
        addition = (HERE / "conformance_toml.py").read_text()
        changed += "\n\n" + addition
        edits.append({"kind": "native typed TOML boundary", "adapter": "conformance_toml.py", "adapter_sha256": digest(addition.encode())})
        edits.append({"kind": "native live typed JSON import", "adapter": "conformance_json.py"})
    restored = changed
    for before, after in reversed(body_edits):
        restored = restored.replace(after, before)
    # A local function's declaration is part of its enclosing function's body.
    for edit in edits:
        if edit["kind"] == "native function declaration":
            restored = restored.replace(edit["after"], edit["before"])
    original_bodies, generated_bodies = bodies(source), bodies(restored, restore_calls=True)
    if any(generated_bodies.get(name) != body for name, body in original_bodies.items()):
        raise ValueError("adapter changed an application function body beyond declared native calls")
    return changed, edits


def annotation_pattern(source: str) -> str | None:
    for node in ast.parse(source).body:
        if (isinstance(node, ast.Assign) and len(node.targets) == 1
                and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "CONFORMANCE_ERROR_PATTERN"):
            call = node.value
            if not isinstance(call, ast.Call) or ast.unparse(call.func) != "re.compile" or len(call.args) != 2 or ast.unparse(call.args[1]) != "re.VERBOSE":
                raise ValueError("unexpected fixed annotation pattern")
            return ast.literal_eval(call.args[0])
    return None


def native_verbose(pattern: str) -> str:
    """Remove Python verbose layout and preserve escaped and bracketed literals for RE2."""
    output = []
    chars = iter(pattern)
    in_class = False
    for char in chars:
        if char == "\\":
            escaped = next(chars)
            if escaped == "s":
                if in_class:
                    raise ValueError("unsupported whitespace escape inside bracketed literal")
                output.append(r"[\x09-\x0d\x1c-\x20\x{85}\p{Z}]")
            else:
                output.extend((char, escaped))
        elif not in_class and char == "#":
            for char in chars:
                if char == "\n":
                    break
        elif not in_class and char in " \t\n\r\v\f":
            continue
        else:
            output.append(char)
            if char == "[":
                in_class = True
            elif char == "]":
                in_class = False
    return "".join(output)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codon", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--applications", nargs="+", choices=FILES, default=list(FILES))
    parser.add_argument("--conformance-scanner", action="store_true", help="also compile the isolated preloaded-line scanner")
    parser.add_argument("--platform-library", type=Path, help="native benchmark platform shared library, required for conformance")
    parser.add_argument("--conformance-classification", action="store_true")
    parser.add_argument("--conformance-grouping", action="store_true")
    parser.add_argument("--conformance-stdout", action="store_true")
    args = parser.parse_args()
    if "conformance_report" in args.applications and args.platform_library is None:
        parser.error("conformance_report requires --platform-library (native Rust OS/TOML boundary)")
    codon = args.codon.resolve(strict=True)
    if digest(codon.read_bytes()) != COMPILER_SHA256:
        parser.error("use the pinned official Codon 0.20.4 Linux x86_64 release")
    libraries = {}
    for name, expected in LIBRARIES.items():
        path = codon.parent.parent / "lib/codon" / name
        actual = digest(path.read_bytes())
        if actual != expected:
            parser.error("pinned Codon library changed: " + name)
        libraries[name] = {"path": str(path), "sha256": actual}
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    source_root = output / "source"
    source_root.mkdir()
    record = {
        "schema": 1, "status": "preparing", "release": "v0.20.4", "compiler": str(codon),
        "compiler_sha256": COMPILER_SHA256, "source_inputs": {}, "transformations": {}, "generated": {}, "application_function_bodies": {}, "generated_application_function_bodies": {},
        "libraries": libraries,
        "commands": [], "binaries": {}, "environment_overrides": {"CODON_PYTHON": "/nonexistent/relar-native-codon-libpython.so", "CODON_PATH": str(source_root)},
        "domain": {"integers": "signed 64-bit; no arbitrary-precision fallback", "duplicate_json_keys": "stock Codon JSON keeps the last value; typed Relar rejects known-field duplicates", "python_interop": False},
    }
    if args.platform_library is not None:
        platform = args.platform_library.resolve(strict=True)
        record["native_platform"] = {"path": str(platform), "sha256": digest(platform.read_bytes()),
                                     "scope": "TOML decoding, child capture/timeouts, temporary workspaces; no report computation or Python interop"}
    for app in args.applications:
        directory = source_root / app
        directory.mkdir()
        for filename in FILES[app]:
            relative = f"{app}/{filename}"
            original = ROOT / "examples" / relative
            data = original.read_bytes()
            transformed, changes = adapt(data.decode(), relative)
            (directory / Path(filename).with_suffix(".codon")).write_text(transformed)
            record["source_inputs"][str(original)] = digest(data)
            record["transformations"][relative] = changes
            record["application_function_bodies"][relative] = bodies(data.decode())
            generated_bodies = bodies(transformed)
            record["generated_application_function_bodies"][relative] = {
                name: generated_bodies[name] for name in record["application_function_bodies"][relative]
            }
    (source_root / "codon_json_helpers.codon").write_bytes((HERE / "json_helpers.py").read_bytes())
    (source_root / "codon_decimal.codon").write_bytes((HERE / "decimal.py").read_bytes())
    (source_root / "codon_sort.codon").write_bytes((HERE / "sort.py").read_bytes())
    if "conformance_report" in args.applications:
        reporter = ROOT / "examples/conformance_report"
        if "from enum import StrEnum, auto" in (reporter / "model.py").read_text():
            native_enum = codon.parent.parent / "lib/codon/stdlib/enum.codon"
            record["native_enum"] = {"path": str(native_enum), "sha256": digest(native_enum.read_bytes())}
            record["source_inputs"][str(native_enum)] = record["native_enum"]["sha256"]
        pattern = annotation_pattern((reporter / "parse.py").read_text())
        if pattern is not None:
            if pattern != annotation_pattern((reporter / "upstream/conformance.py").read_text()):
                raise ValueError("native regex boundary requires the unmodified Ruff pattern")
            adapter = (HERE / "conformance_re.py").read_text()
            (source_root / "codon_conformance_re.codon").write_text(
                adapter + "\nORIGINAL = " + repr(pattern) + "\nNATIVE = " + repr(native_verbose(pattern)) + "\n"
            )
            native_re = codon.parent.parent / "lib/codon/stdlib/re.codon"
            record["native_re"] = {"path": str(native_re), "sha256": digest(native_re.read_bytes())}
        for source, generated in (("conformance_dedent.py", "textwrap.codon"),
                                  ("live_platform.py", "codon_platform.codon"),
                                  ("live_subprocess.py", "subprocess.codon"),
                                  ("live_tempfile.py", "tempfile.codon")):
            (source_root / generated).write_bytes((HERE / source).read_bytes())
    urllib = source_root / "urllib"
    urllib.mkdir()
    (urllib / "__init__.codon").write_text("")
    (urllib / "parse.codon").write_bytes((HERE / "unquote.py").read_bytes())
    for path in HERE.glob("*.py"):
        record["source_inputs"][str(path)] = digest(path.read_bytes())
    native_json = codon.parent.parent / "lib/codon/stdlib/json.codon"
    record["native_json"] = {"path": str(native_json), "sha256": digest(native_json.read_bytes())}
    if "python_catalog" in args.applications:
        record["native_catalog_io"] = {str(path): digest(path.read_bytes()) for path in (
            native_json.with_name("gzip.codon"), native_json.parent / "internal/file.codon")}
    targets = [(app, "__main__") for app in args.applications]
    targets += [(app, "compute") for app in args.applications if "compute.py" in FILES[app]]
    entries = []
    for app, module in targets:
        name = app + ("-compute" if module == "compute" else "")
        entry = source_root / ("run_" + name + ".py")
        entry.write_text(f"import {app}.{module}\n")
        entries.append((name, entry))
    if args.conformance_stdout:
        if "conformance_report" not in args.applications:
            parser.error("--conformance-stdout requires conformance_report")
        original = Path(__file__).with_name("conformance_stdout.py")
        entry = source_root / "conformance_stdout.py"
        entry.write_bytes(original.read_bytes())
        record["source_inputs"][str(original)] = digest(original.read_bytes())
        entries.insert(0, ("conformance_report-stdout", entry))
    if args.conformance_scanner:
        if "conformance_report" not in args.applications:
            parser.error("--conformance-scanner requires the conformance reporter")
        scanner = ROOT / "benchmarks/conformance_scanner.py"
        entry = source_root / "run_conformance_scanner.py"
        entry.write_bytes(scanner.read_bytes())
        record["source_inputs"][str(scanner)] = digest(scanner.read_bytes())
        entries.append(("conformance_report-scanner", entry))
    if args.conformance_classification:
        if "conformance_report" not in args.applications:
            parser.error("--conformance-classification requires the conformance reporter")
        source = ROOT / "benchmarks/conformance_classification.py"
        entry = source_root / "run_conformance_classification.py"
        transformed, edits = adapt(source.read_text(), "conformance_classification.py")
        entry.write_text(transformed)
        record["source_inputs"][str(source)] = digest(source.read_bytes())
        record["transformations"]["conformance_classification.py"] = edits
        entries.append(("conformance_report-classification", entry))
        probe = source_root / "run_conformance_enum_probe.py"
        probe.write_bytes((HERE / "enum_probe.py").read_bytes())
        entries.append(("conformance_report-enum-probe", probe))
    if args.conformance_grouping:
        if "conformance_report" not in args.applications:
            parser.error("--conformance-grouping requires the conformance reporter")
        source = ROOT / "benchmarks/conformance_grouping.py"
        entry = source_root / "run_conformance_grouping.py"
        transformed, edits = adapt(source.read_text(), "conformance_grouping.py")
        entry.write_text(transformed)
        record["source_inputs"][str(source)] = digest(source.read_bytes())
        record["transformations"]["conformance_grouping.py"] = edits
        entries.append(("conformance_report-grouping", entry))
    record["generated"] = {
        str(path.relative_to(source_root)): digest(path.read_bytes())
        for path in sorted(source_root.rglob("*")) if path.is_file()
    }
    for name, entry in entries:
        binary = output / name
        command = [str(codon), "build", "-release", "-numerics=py", "-disable-native", "-exe", "-o", str(binary), str(entry)]
        if args.platform_library is not None:
            command.extend(["--linker-flags", shlex.join([str(platform), "-Wl,-rpath," + str(platform.parent)])])
        print("Building " + name, flush=True)
        result = subprocess.run(command, cwd=source_root, env={**os.environ, **record["environment_overrides"]}, capture_output=True)
        captures = {}
        for stream in ("stdout", "stderr"):
            data = getattr(result, stream)
            path = output / (name + "." + stream)
            path.write_bytes(data)
            captures[stream] = {"path": str(path), "sha256": digest(data)}
        record["commands"].append({"name": name, "command": command, "returncode": result.returncode, **captures})
        if result.returncode:
            record["status"] = "failed"
            (output / "build-record.json").write_text(json.dumps(record, indent=2) + "\n")
            raise SystemExit(result.stderr.decode())
        record["binaries"][name] = {"path": str(binary), "sha256": digest(binary.read_bytes())}
        print("Built " + name, flush=True)
    record["status"] = "passed"
    (output / "build-record.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
