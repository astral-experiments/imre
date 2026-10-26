"""Compare the unchanged FileStorage wheel with the recorded CPython filename and save cases."""
import argparse
from contextlib import chdir
import hashlib
from io import BytesIO
import json
from pathlib import Path
import runpy
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from suite_native import NativeCompileError, Toolchain
from werkzeug_paths import MyStr, NativePaths, PathValue, Refused


def filenames():
    return {"str": "café.txt", "str-subclass": MyStr("class.txt"), "valid-utf8": "🧪.txt".encode(),
            "invalid-bytes": b"bad\xff\x80.txt", "incomplete-utf8": b"x\xe2\x82", "nul": b"x\0y",
            "angle": b"<stdin>", "empty": b"", "path-str": PathValue(MyStr("path.txt")),
            "path-bytes": PathValue(b"p\xff"), "pathlib": Path("fólder/a"), "invalid-result": PathValue(17),
            "raising-path": Refused(), "wrong-type": 17}


def render(name):
    if name is None:
        return None
    return {"type": type(name).__name__, "codepoints": list(map(ord, name)),
            "bytes": name.encode("utf-8", "surrogateescape").hex()}


def observe(case):
    try:
        return case()
    except NativeCompileError as error:
        return {"compiler_blocker": str(error)}
    except (OSError, TypeError, AttributeError, NotImplementedError) as error:
        return {"error": [type(error).__name__, str(error)]}


def exercise(module, source, provider):
    rows = {}
    events = []
    def binding(operation):
        def invoke(operand):
            caller = sys._getframe(1)
            assert Path(caller.f_code.co_filename) == source
            event = {"operation": operation, "line": caller.f_lineno, "executed_in_imre": False}
            events.append(event)
            return provider(operation, operand, event)
        return invoke

    def record(key, case, *, count_path=True, save=False):
        events.clear()
        row = observe(case)
        target = [e for e in events if not save or e["operation"] == "fspath"]
        row["wheel_calls"] = [[e["operation"], e["line"]] for e in target]
        if count_path:
            counts = [e["descriptor_calls"] for e in target if "descriptor_calls" in e]
            row["path_calls"] = sum(counts) if counts else None
        rows[key] = {"observation": row, "calls": events.copy()}

    def upload(stream, value, explicit):
        stored = module.FileStorage(stream, filename=value if explicit else None)
        try:
            original = value.value if isinstance(value, PathValue) else value
            return {"filename": render(stored.filename), "input_identity": stored.filename is original,
                    "body": stored.read().hex(), "same_stream": stored.stream is stream}
        finally:
            stored.close()

    def filename_only(stream):
        stored = module.FileStorage(stream)
        try:
            return {"filename": render(stored.filename)}
        finally:
            stored.close()

    def save(destination):
        stored = module.FileStorage(BytesIO(b"payload"), filename="test.bin")
        try:
            stored.save(destination)
            if isinstance(destination, BytesIO):
                body = destination.getvalue()
            else:
                path = destination.value if isinstance(destination, PathValue) else destination
                body = Path(path).read_bytes()
            return {"body": body.hex()}
        finally:
            stored.close()

    with patch.object(module, "fsdecode", binding("fsdecode")), patch.object(module, "fspath", binding("fspath")):
        for mode in ("explicit", "stream-name"):
            for label, value in filenames().items():
                with BytesIO(b"body") as stream:
                    if mode == "stream-name":
                        stream.name = value
                    record(f"{mode}:{label}", lambda: upload(stream, value, mode == "explicit"))
        with BytesIO(b"no-name") as stream:
            record("stream-name:absent", lambda: filename_only(stream), count_path=False)
        name = b"actual-\xff.bin"
        open(name, "wb").close()
        with open(name, "rb") as stream:
            record("real-byte-filename", lambda: filename_only(stream), count_path=False)
        with BytesIO() as stream:
            for label, dest in {"pathlib": Path("dest-a"), "path-str": PathValue("dest-b"),
                                "path-bytes": PathValue(b"dest-c"), "path-invalid": PathValue(17),
                                "stream": stream}.items():
                record("save:" + label, lambda: save(dest), save=True)
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "wheelhouse", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.platform != "linux":
        parser.error("use Linux and CPython 3.14.5")
    expected = json.loads((HERE / "filesystem-codec-linux.json").read_bytes())
    args.output.mkdir(parents=True, exist_ok=True)
    output = args.output.resolve()
    stage = runpy.run_path(str(HERE.parents[2] / "flask/gate.py"))["stage"]
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        site = root / "site"
        site.mkdir()
        assert stage(args.wheelhouse.resolve(), site) == expected["wheels"]
        sys.path.insert(0, str(site))
        from werkzeug.datastructures import file_storage
        source = site / expected["wheel_source"]
        assert hashlib.sha256(source.read_bytes()).hexdigest() == expected["wheel_source_sha256"]
        assert Path(file_storage.FileStorage.__init__.__code__.co_filename) == source
        for reachable in (False, True):
            for level in (0, 3):
                name = f"{'reachable' if reachable else 'ordinary'}-O{level}"
                tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level, reachable)
                with tempfile.TemporaryDirectory(dir=root) as work, chdir(work):
                    rows = exercise(file_storage, source, NativePaths(tools, Path(work)))
                assert rows.keys() == expected["rows"].keys()
                (output / (name + ".json")).write_text(json.dumps(rows, indent=2) + "\n")
                for key, row in rows.items():
                    baseline = expected["rows"][key]
                    assert row["observation"] == baseline.get("observed", baseline["reference"]), (name, key, row)
                    assert row["calls"] == baseline["calls"], (name, key, row["calls"])
                print(name, sum(row["observation"] == expected["rows"][key]["reference"]
                                for key, row in rows.items()), "/", len(rows), flush=True)


if __name__ == "__main__":
    main()
