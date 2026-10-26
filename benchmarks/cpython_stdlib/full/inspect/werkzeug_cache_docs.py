"""Compare Werkzeug's actual cache-control descriptor docs with a native inspect.cleandoc."""
import argparse
import inspect
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
sys.path.insert(0, str(HERE.parents[2] / "flask"))
from gate import digest, stage
from suite_native import Toolchain
from cleandoc_cases import native_clean

EXPLICIT = {
    "empty": "", "tabs": "\n\tTitle\n\t  details\n\n", "crlf": " A\r\n  B\r\n",
    "nul": "\0 x\n  y\0", "unicode_space": "\u2003π\n  \u00a0雪\n",
    "space_only_late": "head\n    \n", "generated_custom_type": None,
}


def trace(site, binary):
    # Each process imports the unchanged wheel afresh, including all class-level property factories.
    sys.path.insert(0, str(site))
    source = site / "werkzeug/datastructures/cache_control.py"
    original = inspect.cleandoc
    phase = "import"
    calls = []

    def clean(doc):
        frame = sys._getframe(1)
        if frame.f_code.co_filename != str(source) or frame.f_code.co_name != "cache_control_property":
            return original(doc)
        result = original(doc) if binary is None else native_clean(binary, site.parent / "doc.utf8", doc)
        calls.append({"phase": phase, "key": frame.f_locals["key"], "input": doc, "result": result})
        return result

    with patch.object(inspect, "cleandoc", clean):
        from werkzeug.datastructures import cache_control as cc

        assert Path(cc.__file__) == source
        classes = (cc._CacheControl, cc.RequestCacheControl, cc.ResponseCacheControl)
        properties = {}
        for call in calls:
            name = call["key"].replace("-", "_")
            owner, = (cls for cls in classes if isinstance(prop := vars(cls).get(name), property)
                      and prop.__doc__ == call["result"])
            properties[f"{owner.__name__}.{name}"] = vars(owner)[name].__doc__
        assert len(properties) == len(calls) == 17
        phase = "explicit"
        for key, raw in EXPLICIT.items():
            prop = cc.cache_control_property(key, "empty", float, doc=raw)
            assert calls[-1]["key"] == key and prop.__doc__ == calls[-1]["result"]
    return {
        "cpython": sys.version.split()[0], "werkzeug_file_sha256": digest(source),
        "property_docs": properties, "calls": calls,
    }


def observed(site, binary=None):
    command = [sys.executable, "-ESB", __file__, "--trace", site]
    if binary is not None:
        command.append(binary)
    run = subprocess.run(command, capture_output=True, timeout=60)
    assert run.returncode == 0 and not run.stderr, (run.returncode, run.stderr)
    return json.loads(run.stdout)


def main():
    if sys.argv[1:2] == ["--trace"]:
        print(json.dumps(trace(Path(sys.argv[2]), Path(sys.argv[3]) if len(sys.argv) == 4 else None)))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "wheelhouse", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--reference", type=Path, help="independently recorded CPython factory trace")
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use pinned CPython 3.14.5")
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        site = root / "site"
        site.mkdir()
        wheels = stage(args.wheelhouse, site)
        reference = observed(site)
        if args.reference:
            assert {**reference, "wheels": wheels} == json.loads(args.reference.read_bytes())
        modes = {}
        for level in (0, 3):
            for reachable in (False, True):
                tool = Toolchain(args.imre, args.runtime, args.clang, level, reachable)
                binary = tool.build((HERE / "cleandoc_worker.py").read_bytes(), root)
                actual = observed(site, binary)
                assert actual == reference, (level, reachable, actual)
                name = f"{'reachable' if reachable else 'ordinary'}-O{level}"
                modes[name] = {"calls_matched": len(actual["calls"]), "properties_matched": len(actual["property_docs"])}
                print(name, modes[name], flush=True)
    report = {**reference, "wheels": wheels, "native_modes": modes,
              "imre_sha256": digest(args.imre), "runtime_sha256": digest(args.runtime),
              "native_worker_sha256": digest(HERE / "cleandoc_worker.py")}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
