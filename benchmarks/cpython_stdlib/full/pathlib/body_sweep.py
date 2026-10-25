"""Probe complete original methods under their upstream CPython fixture lifecycles."""

import argparse
import ast
import collections
import functools
import hashlib
import inspect
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from contextlib import chdir
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parents[1]
PACKAGE = BENCH / "upstream/Lib/test/test_pathlib"
sys.path.insert(0, str(BENCH))
from suite_evidence import reference_cases
from suite_inventory import load_suite, verify_interpreter_tests
from suite_native import Toolchain, NativeCompileError, run_in_fixtures
from upstream import verify_upstream

FIVE = {"Path", "PosixPath", "PurePath", "PurePosixPath", "PureWindowsPath"}


def shape(value, depth=0):
    """Record assertion operand types only; neither expected nor actual values are used."""
    tp = type(value)
    if depth < 3 and tp in (list, tuple, set, frozenset):
        items = [shape(x, depth + 1) for x in tuple(value)[:8]]
        return tp.__name__ + "[" + ",".join(items if tp is tuple else sorted(set(items))) + "]"
    if tp is type:
        return "type:" + value.__module__ + "." + value.__qualname__
    return tp.__name__ if tp.__module__ == "builtins" else tp.__module__ + "." + tp.__qualname__


def assertion_shapes(case, original):
    calls = collections.defaultdict(set)

    def record(frame, event, _):
        code = frame.f_code
        if (
            event == "call"
            and frame.f_globals.get("__name__") == "unittest.case"
            and code.co_name.startswith("assert")
        ):
            calls[code.co_name].add(
                tuple(
                    shape(frame.f_locals[n])
                    for n in code.co_varnames[: code.co_argcount]
                    if n not in ("self", "msg")
                )
            )

    previous = sys.getprofile()
    try:
        sys.setprofile(record)
        original()
    finally:
        sys.setprofile(previous)
    return calls


def type_name(raw, ctor):
    if raw in ("str", "bytes", "bool", "int", "float"):
        return raw
    if raw == "NoneType":
        return "None"
    if raw in ("pathlib.PosixPath", "pathlib.PurePosixPath", "pathlib.PureWindowsPath"):
        if ctor in FIVE:
            return ctor
        return raw.split(".")[-1]
    if raw.startswith(("list[", "set[")) and raw.endswith("]"):
        inner = raw[raw.index("[") + 1 : -1]
        if not inner:
            return raw.split("[")[0] + "[?]"
        arg = type_name(inner, ctor)
        if arg:
            return raw.split("[")[0] + "[" + arg + "]"
    return None


def single(types):
    types = set(types)
    for t in tuple(types):
        if (
            t
            and "[?]" in t
            and any(x and x.startswith(t.split("[")[0] + "[") and "?" not in x for x in types)
        ):
            types.discard(t)
    return (
        next(iter(types))
        if len(types) == 1 and None not in types and "?" not in next(iter(types))
        else None
    )


@functools.cache
def module_data(module):
    source = PACKAGE / ("/".join(module.split(".")[2:]) + ".py")
    data = source.read_bytes()
    tree = ast.parse(data)
    lines = data.splitlines(keepends=True)
    return source, tree, lines


def raw(lines, node, decorators=False):
    first = (
        min([node.lineno, *[d.lineno for d in getattr(node, "decorator_list", ())]])
        if decorators
        else node.lineno
    )
    return b"".join(lines[first - 1 : node.end_lineno])


def declaration(type_, name):
    owner = next(base for base in type_.__mro__ if name in vars(base))
    path, tree, lines = module_data(owner.__module__)
    cls = next(n for n in ast.walk(tree) if isinstance(n, ast.ClassDef) and n.name == owner.__name__)
    fun = next((n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == name), None)
    return owner, path, tree, lines, fun


def checks(shape, attrs, ctor):
    s = ""
    for name in sorted(attrs & {"assertEqual", "assertNotEqual"}):
        pairs = shape.get(name, ())
        if not pairs:
            continue
        a = single([type_name(x[0], ctor) for x in pairs])
        b = single([type_name(x[1], ctor) for x in pairs])
        if not a or not b or a != b:
            continue
        s += f"    def {name}(self, left: {a}, right: {b}, msg: str | None = None) -> None:\n"
        expected = name == "assertEqual"
        if a.startswith("list["):
            s += "        equal = len(left) == len(right)\n        if equal:\n            for index in range(len(left)):\n                if left[index] != right[index]:\n                    equal = False\n                    break\n"
            pred = "not equal" if expected else "equal"
        else:
            pred = "left != right" if expected else "left == right"
        s += f'        if {pred}:\n            raise AssertionError("different from upstream assertion")\n'
    for name in sorted(attrs & {"assertTrue", "assertFalse"}):
        vals = shape.get(name, ())
        ty = single(type_name(x[0], ctor) for x in vals if x)
        if not ty:
            continue
        pred = "not value" if name == "assertTrue" else "value"
        s += f'    def {name}(self, value: {ty}, msg: str | None = None) -> None:\n        if {pred}:\n            raise AssertionError("unexpected truth value")\n'
    return s


def relative_imports(tree, lines, uses):
    statements = []
    for node in tree.body:
        if not isinstance(node, (ast.Import, ast.ImportFrom)):
            continue
        if not any((n.asname or n.name.split(".")[0]) in uses for n in node.names):
            continue
        if isinstance(node, ast.ImportFrom) and node.level:
            names = ", ".join(n.name + (" as " + n.asname if n.asname else "") for n in node.names)
            statements.append(
                "from test.test_pathlib." + (node.module or "") + " import " + names + "\n"
            )
        else:
            statements.append(raw(lines, node).decode())
    return "".join(dict.fromkeys(statements))


def fixture(case, tree, lines, method, shape):
    cls = type(case)
    helpers = []
    todo = [method]
    seen = set()
    uses = set()
    attrs = set()
    arities = set()
    aliases = {"P"}
    while todo:
        current = todo.pop()
        for n in ast.walk(current):
            if isinstance(n, ast.Name) and isinstance(n.ctx, ast.Load):
                uses.add(n.id)
            if (
                isinstance(n, ast.Assign)
                and isinstance(n.value, ast.Attribute)
                and n.value.attr == "cls"
            ):
                aliases.update(t.id for t in n.targets if isinstance(t, ast.Name))
            if isinstance(n, ast.Call) and (
                (isinstance(n.func, ast.Name) and n.func.id in aliases)
                or (
                    isinstance(n.func, ast.Attribute)
                    and n.func.attr == "cls"
                    and isinstance(n.func.value, ast.Name)
                    and n.func.value.id == "self"
                )
            ):
                arities.add(len(n.args))
            if not (
                isinstance(n, ast.Attribute) and isinstance(n.value, ast.Name) and n.value.id == "self"
            ):
                continue
            name = n.attr
            attrs.add(name)
            if name in seen or name == method.name or not name.startswith("_check"):
                continue
            seen.add(name)
            try:
                _, _, _, h_lines, h = declaration(cls, name)
            except (StopIteration, AttributeError, FileNotFoundError):
                continue
            if h is not None:
                helpers.append(raw(h_lines, h, True))
                todo.append(h)
    typ = getattr(case, "cls", None)
    ctor = getattr(typ, "__name__", None)
    mod = getattr(typ, "__module__", "")
    pre = "import unittest\nimport pathlib\nfrom pathlib import Path, PurePath, PosixPath, PurePosixPath, PureWindowsPath\n"
    pre += relative_imports(tree, lines, uses)
    fields = {}
    ctor_code = ""
    guard = []
    if "cls" in attrs:
        if ctor in FIVE and mod == "pathlib":
            pass
        elif typ is not None and mod.startswith("test.test_pathlib.support."):
            pre += f"from {mod} import {ctor}\n"
            guard.append("custom path fixture " + mod + "." + ctor)
        elif typ is not None and mod.startswith("test.test_pathlib."):
            _, dt, dl = module_data(mod)
            outer = next(
                n for n in ast.walk(dt) if isinstance(n, ast.ClassDef) and n.name == cls.__name__
            )
            nested = next(
                (n for n in outer.body if isinstance(n, ast.ClassDef) and n.name == ctor), None
            )
            if nested is not None:
                pre += "class " + cls.__name__ + ":\n" + raw(dl, nested, True).decode() + "\n"
                ctor = cls.__name__ + "." + ctor
            guard.append("exact user subclass " + mod + "." + str(ctor))
        if ctor:
            lo = min(arities, default=1)
            hi = max(arities, default=1)
            params = ",".join(f"a{i}: str" + (" = ''" if i >= lo else "") for i in range(hi))
            values = ",".join(f"a{i}" for i in range(hi))
            ctor_code = f"    def cls(self{', ' if params else ''}{params}) -> {ctor}:\n        return {ctor}({values})\n"
    for name in sorted(attrs):
        if name in ("cls", "ground") or name.startswith(("assert", "test_", "_")):
            continue
        value = getattr(case, name, None)
        if type(value) in (bool, str, int):
            fields[name] = (type(value).__name__, repr(value))
        elif type(value).__name__ in FIVE and type(value).__module__ == "pathlib":
            fields[name] = ("Path", f"Path({os.fspath(value)!r})")
        elif value is not None and not callable(value):
            guard.append(f"fixture field {name}: {type(value).__module__}.{type(value).__qualname__}")
    if "ground" in attrs and hasattr(case, "ground"):
        ground = case.ground
        pre += (
            "class Ground:\n    can_symlink: bool\n    def __init__(self) -> None:\n        self.can_symlink = "
            + repr(bool(getattr(ground, "can_symlink", False)))
            + "\n"
        )
        fields["ground"] = ("Ground", "Ground()")
    assertions = checks(shape, attrs, getattr(typ, "__name__", None))
    inherited = not assertions
    pre += "class NativeCase" + ("(unittest.TestCase)" if inherited else "") + ":\n"
    pre += "".join(f"    {n}: {ty}\n" for n, (ty, val) in fields.items())
    pre += (
        "    def __init__(self) -> None:\n"
        + ("        super().__init__()\n" if inherited else "        pass\n")
        + "".join(f"        self.{n} = {v}\n" for n, (_, v) in fields.items())
    )
    pre += ctor_code + assertions
    return pre.encode() + b"\n" + b"\n".join(helpers) + b"\n", guard


def run_all(args):
    import test
    from test import support

    oracle = Path(test.__file__).resolve().parents[1]
    imre, runtime, clang = (p.resolve() for p in (args.imre, args.runtime, args.clang))
    ref = json.loads((HERE / "reference-linux.json").read_bytes())
    if verify_upstream() != ref["cpython_commit"]:
        raise ValueError("reference and test blobs disagree")
    pin = json.loads((BENCH / "UPSTREAM.json").read_bytes())
    verify_interpreter_tests(
        {"files": {n: h for n, h in pin["files"].items() if n.startswith("Lib/test/test_pathlib/")}}
    )
    refs = reference_cases(ref)
    if any(n != 1 for _, n in refs):
        raise ValueError("review fixtures for repeated pathlib IDs")
    idlist = [
        name
        for (name, _), v in refs.items()
        if v == "pass" and (not args.pattern or re.search(args.pattern, name))
    ]
    if not idlist:
        raise ValueError("no matching reference passes")
    support.use_resources = ["all"]
    root = args.work.resolve()
    root.mkdir(parents=True, exist_ok=True)
    suite_root = root / "test"
    suite_root.mkdir(exist_ok=True)
    shutil.copyfile(oracle / "test/__init__.py", suite_root / "__init__.py")
    for child in ("support", "test_pathlib"):
        shutil.copytree(
            oracle / "test" / child,
            suite_root / child,
            dirs_exist_ok=True,
            ignore=shutil.ignore_patterns("__pycache__"),
        )
    os.environ["IMRE_ENCODED_FLAGS"] = "--module-root\x1f" + str(root)
    with tempfile.TemporaryDirectory(prefix="plb-") as td, chdir(td):
        shapes, fail, skip = run_in_fixtures(
            load_suite("test.test_pathlib"), dict.fromkeys(idlist, assertion_shapes)
        )
        if fail or skip or set(shapes) != set(idlist):
            raise RuntimeError(("reference operand types", fail, skip))
        for level in args.level:
            tools = Toolchain(imre, runtime, clang, level)
            proof = root / f"progress-O{level}.jsonl"
            cache = {}
            completed = {}

            def go(case, original):
                case_id = case.id()
                name = case._testMethodName
                rec = {"id": case_id, "opt_level": level, "instance": type(case).__qualname__}

                def record(status, **kw):
                    rec.update(status=status, **kw)
                    with proof.open("a", encoding="utf8") as stream:
                        stream.write(json.dumps(rec, sort_keys=True) + "\n")
                    completed[case_id] = rec
                    return rec

                try:
                    owner, src, tree, lines, method = declaration(type(case), name)
                    if method is None:
                        return record(
                            "original fixture binding",
                            diagnostic="registered name is an assignment, not a source function",
                            owner=owner.__qualname__,
                        )
                    original_code = original.__func__.__code__
                    rec.update(
                        source=src.relative_to(PACKAGE).as_posix(),
                        upstream_line=method.lineno,
                        owner=owner.__qualname__,
                    )
                    decorators = [ast.unparse(d) for d in method.decorator_list]
                    body = raw(lines, method)
                    rec["method_sha256"] = hashlib.sha256(body).hexdigest()
                    if (
                        original_code.co_name != name
                        or original_code is not inspect.unwrap(original).__code__
                    ):
                        return record(
                            "original fixture wrapper",
                            diagnostic="bound callable " + original_code.co_qualname,
                            wrapper_line=original_code.co_firstlineno,
                            decorators=decorators,
                        )
                    identity = all(
                        d in ("needs_posix", "needs_windows", "needs_symlinks") for d in decorators
                    )
                    if decorators and identity:
                        module = inspect.getmodule(original)
                        for registry in decorators:
                            if name not in getattr(module, "_tests_needing_" + registry[6:]):
                                raise ValueError("original gate not registered: " + registry)
                    pre, guards = fixture(case, tree, lines, method, shapes[case_id][0])
                    if guards:
                        rec["fixture_limits"] = guards
                    if decorators and not identity:
                        body = raw(lines, method, True)
                    entry = (
                        pre
                        + body
                        + f'\ncase = NativeCase()\ncase.{name}()\nprint("native method finished")\n'.encode()
                    )
                    digest = hashlib.sha256(entry).hexdigest()
                    d = root / f"level_{level}" / f"case_{digest}"
                    d.mkdir(parents=True, exist_ok=True)
                    (d.parent / "__init__.py").touch()
                    (d / "__init__.py").touch()
                    artifact = cache.get(digest)
                    if artifact is None:
                        try:
                            artifact = tools.build(entry, d)
                        except NativeCompileError as exc:
                            artifact = exc
                        if isinstance(artifact, NativeCompileError) or args.keep_binaries:
                            cache[digest] = artifact
                    if isinstance(artifact, NativeCompileError):
                        msg = str(artifact).replace(str(d), "<case>").strip()
                        messages = [
                            x.strip()
                            for x in msg.splitlines()
                            if x.strip() and not x.lstrip().startswith("Compiling ")
                        ]
                        line = next((x for x in messages if "Imre:" in x or "error[" in x), messages[0])
                        m = re.search(r"<case>/case.py:(\d+):(\d+)", line)
                        start = pre.count(b"\n") + 1
                        n = int(m[1]) if m else 0
                        stage = (
                            "unchanged body"
                            if start <= n < start + body.count(b"\n")
                            else "fixture or dependency"
                        )
                        diagnostic = line.replace(str(oracle), "<cpython>/Lib").replace(
                            str(root), "<module-root>"
                        )
                        if stage == "unchanged body":
                            rec.update(
                                failing_line=(
                                    min([method.lineno, *[a.lineno for a in method.decorator_list]])
                                    if decorators and not identity
                                    else method.lineno
                                )
                                + n
                                - start
                            )
                        return record("compile blocker", stage=stage, diagnostic=diagnostic)
                    try:
                        run = subprocess.run(
                            [artifact], capture_output=True, text=True, encoding="utf8", timeout=60
                        )
                    except subprocess.TimeoutExpired:
                        return record("native timeout", stage="execution")
                    finally:
                        if not args.keep_binaries:
                            artifact.unlink()
                    if run.returncode or run.stdout != "native method finished\n" or run.stderr:
                        return record(
                            "native failure",
                            code=run.returncode,
                            stderr=run.stderr.strip()[-520:],
                            stdout=run.stdout.strip()[-200:],
                        )
                    return record("native candidate", guarded=bool(guards))
                except (
                    AttributeError,
                    IndexError,
                    KeyError,
                    StopIteration,
                    FileNotFoundError,
                    TypeError,
                    ValueError,
                ) as exc:
                    return record("fixture preparation", diagnostic=f"{type(exc).__name__}: {exc}")

            with proof.open("w", encoding="utf8"):
                pass
            _, fail, skip = run_in_fixtures(load_suite("test.test_pathlib"), dict.fromkeys(idlist, go))
            print(
                level,
                len(completed),
                collections.Counter(r["status"] for r in completed.values()),
                len(fail),
                len(skip),
                flush=True,
            )
            if fail or skip:
                print(fail, skip, flush=True)
            (root / f"outcome-O{level}.json").write_text(
                json.dumps(
                    {"completed": completed, "failures": fail, "skips": skip}, indent=2, sort_keys=True
                )
                + "\n",
                encoding="utf8",
            )
            if fail or skip or set(completed) != set(idlist):
                raise RuntimeError(("native fixture lifecycle", level, fail, skip))


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--pattern", help="only probe matching IDs (diagnostics only)")
    p.add_argument("--level", type=int, choices=(0, 3), nargs="+", default=[0, 3])
    p.add_argument(
        "--keep-binaries",
        action="store_true",
        help="keep disposable linked executables alongside generated sources",
    )
    for name in ("imre", "runtime", "clang", "work"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--cpython-source", type=Path)
    args = p.parse_args()
    if (
        sys.version_info[:3] != (3, 14, 5)
        or sys.platform != "linux"
        or not sys.flags.warn_default_encoding
    ):
        p.error("use the pinned CPython 3.14.5 on Linux with -X warn_default_encoding")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    run_all(args)
