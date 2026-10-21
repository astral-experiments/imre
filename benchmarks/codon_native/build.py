"""Prepare explicit native Codon adapters and build the shared tool algorithms."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
COMPILER_SHA256 = "cde8229e41a67e87edce088fbbd851acdf1b62e913677bb7cd9a9ce5d61c5350"
LIBRARIES = {
    "libcodonc.so": "8aeedf95ba48fbcb7be24b9acb2e213a86b7773c46c40d05eee54c2526edc4b0",
    "libcodonrt.so": "b7dbed21f9bd086d6331581b0bc330e48abab14766e3a69fb970c9855baf878d",
}
FILES = {
    "conformance_report": ["__init__.py", "__main__.py", "compute.py", "app.py", "model.py", "parse.py", "render.py"],
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
            before = lines[node.lineno - 1]
            if before.count("*, ") != 1 or node.body[0].lineno != node.lineno + 1:
                raise ValueError("expected one-line keyword-only function declaration")
            after = before.replace("*, ", "")
            lines[node.lineno - 1] = after
            edits.append({"kind": "native function declaration", "line": node.lineno, "before": before, "after": after})
        if not isinstance(node, ast.ClassDef):
            continue
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
    changed = "".join(lines)
    if any(node.func.id == "int" for node in calls):
        changed = "from codon_decimal import parse_int as native_python_int\n\n" + changed
    if any(node.func.id == "sorted" for node in calls):
        changed = "from codon_sort import sort_with_key as native_python_sorted\n\n" + changed
    dataclass_import = "from dataclasses import dataclass\n"
    if dataclass_import in changed:
        if changed.count(dataclass_import) != 1:
            raise ValueError("unexpected dataclass import")
        changed = changed.replace(dataclass_import, "")
        edits.append({"kind": "native builtin dataclass import", "before": dataclass_import, "after": ""})
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
    original_bodies, generated_bodies = bodies(source), bodies(changed, restore_calls=True)
    if any(generated_bodies.get(name) != body for name, body in original_bodies.items()):
        raise ValueError("adapter changed an application function body beyond declared native calls")
    return changed, edits


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codon", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--applications", nargs="+", choices=FILES, default=list(FILES))
    args = parser.parse_args()
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
    urllib = source_root / "urllib"
    urllib.mkdir()
    (urllib / "__init__.codon").write_text("")
    (urllib / "parse.codon").write_bytes((HERE / "unquote.py").read_bytes())
    for path in HERE.glob("*.py"):
        record["source_inputs"][str(path)] = digest(path.read_bytes())
    native_json = codon.parent.parent / "lib/codon/stdlib/json.codon"
    record["native_json"] = {"path": str(native_json), "sha256": digest(native_json.read_bytes())}
    targets = [(app, "__main__") for app in args.applications]
    targets += [(app, "compute") for app in args.applications if "compute.py" in FILES[app]]
    entries = []
    for app, module in targets:
        name = app + ("-compute" if module == "compute" else "")
        entry = source_root / ("run_" + name + ".py")
        entry.write_text(f"import {app}.{module}\n")
        entries.append((name, entry))
    record["generated"] = {
        str(path.relative_to(source_root)): digest(path.read_bytes())
        for path in sorted(source_root.rglob("*")) if path.is_file()
    }
    for name, entry in entries:
        binary = output / name
        command = [str(codon), "build", "-release", "-numerics=py", "-disable-native", "-exe", "-o", str(binary), str(entry)]
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
