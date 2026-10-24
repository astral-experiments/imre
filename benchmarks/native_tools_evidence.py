"""Package and replay native-tool evidence without the original machine paths.

package --audit AUDIT.json --codon-license LICENSE [--source-binding BINDING.json] --output-dir NEW
replay ARCHIVE --output NEW-RESULT.json

Records and captured inputs remain byte-for-byte unchanged. Recorded paths are
identities in an index, never destinations to recreate. Replay executes only
the retained Python upstream oracles, not benchmark binaries or native tools.
"""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import runpy
import sys
import tempfile
from types import SimpleNamespace


def sha(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    return json.loads(Path(path).read_bytes())


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n")


def safe_relative(name):
    path = Path(name)
    assert not path.is_absolute() and name and all(part not in (".", "..") for part in path.parts), name
    return path


def package(args):
    audit_path = args.audit.resolve(strict=True)
    audit = load(audit_path)
    assert audit["status"] == "passed" and audit["input_guards_passed"]
    guards = dict(audit["guarded_inputs"])
    guards[str(audit_path)] = sha(audit_path.read_bytes())
    roots = [Path(path).parents[1] for path in guards if path.endswith("/benchmarks/native_tools_audit.py")]
    assert len(roots) == 1
    root = roots[0]
    binding = {}
    if args.source_binding:
        source_binding = load(args.source_binding)
        assert source_binding["status"] == "passed"
        assert source_binding["audit_sha256"] == guards[str(audit_path)]
        binding = source_binding["original_to_snapshot"]
        guards[str(args.source_binding.resolve())] = sha(args.source_binding.read_bytes())
    for item in [args.codon_license, *args.extra]:
        path = item.resolve(strict=True)
        if path.is_dir():
            selected = sorted(path.rglob("*"))
            assert all(not item.is_symlink() for item in selected)
            selected = [item for item in selected if item.is_file()]
        else:
            selected = [path]
        for item in selected:
            expected = sha(item.read_bytes())
            assert guards.setdefault(str(item), expected) == expected
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        path = root / name
        guards.setdefault(str(path), sha(path.read_bytes()))
    destination = args.output_dir.resolve()
    assert not destination.exists()
    assert not destination.is_relative_to(root)
    assert all(not destination.is_relative_to(Path(path)) for path in audit["guarded_trees"])
    assert all(not Path(path).is_relative_to(destination) for path in guards)
    destination.mkdir(parents=True)
    (destination / "blobs").mkdir()
    index = {"schema": 1, "kind": "native-tools-evidence", "source_root": str(root),
             "audit": str(audit_path), "records": audit["records"], "files": {}, "external": {},
             "trees": audit["guarded_trees"], "tools": {}, "licenses": [],
             "supplemental": [str(path.resolve()) for path in args.extra],
             "codon_license": {"path": str(args.codon_license.resolve()),
                               "source": "https://github.com/exaloop/codon/blob/5eb079d6f2418b11a5f85346e12b1afd33f32132/LICENSE"}}
    assert guards[str(args.codon_license.resolve())] == "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"
    for original, expected in sorted(guards.items()):
        bound = binding.get(original)
        path = Path(bound["snapshot"] if bound else original)
        assert path.is_file(), path
        if bound:
            assert bound["sha256"] == expected
        data = path.read_bytes()
        assert sha(data) == expected, (original, "frozen source/retained file differs")
        identity = {"sha256": expected, "bytes": len(data)}
        if data.startswith((b"\x7fELF", b"!<arch>\n", b"BC\xc0\xde", b"\xde\xc0\x17\x0b")):
            index["external"][original] = {**identity, "reason": "native executable, library, or compiled archive; not redistributed"}
        else:
            blob = "blobs/" + expected
            target = destination / blob
            if not target.exists():
                target.write_bytes(data)
            index["files"][original] = {**identity, "blob": blob}
            if "LICENSE" in Path(original).name or original == str(args.codon_license.resolve()):
                index["licenses"].append(original)
    for name in ("native_tools_evidence.py", "native_tools_audit.py"):
        data = Path(__file__).with_name(name).read_bytes()
        (destination / name).write_bytes(data)
        index["tools"][name] = {"sha256": sha(data), "bytes": len(data)}
    (destination / "README.md").write_text(
        "# Portable native-tool evidence\n\n"
        "Run `python3.14 -B native_tools_evidence.py replay . --output /tmp/replay.json`. "
        "Use CPython 3.14 or newer with assertions enabled. No network, checkout, Git, "
        "native compiler, or benchmark executable is needed.\n\n"
        "The index maps original path identities to immutable content-addressed blobs. "
        "Reports, commands, manifests, and raw ty streams retain their original bytes. "
        "The replay reconstructs upstream outputs and checks every timing capture, "
        "balanced order, statistic, source binding, and shared-mode input.\n\n"
        "Native binaries and toolchains under `external` were hashed by the live audit "
        "and package creation. They are deliberately excluded and are **not rehashed "
        "by portable replay**. This verifies retained evidence, not a new benchmark run. "
        "License texts are indexed under `licenses`; upstream revisions and copyright "
        "notices remain in the source/provenance blobs.\n"
    )
    write(destination / "index.json", index)
    return {"status": "passed", "archive": str(destination), "retained_files": len(index["files"]),
            "excluded_artifacts": len(index["external"]), "unique_blobs": len(list((destination / "blobs").iterdir())),
            "index_sha256": sha((destination / "index.json").read_bytes())}


class Archive:
    """Resolve only indexed evidence identities; no access to original paths."""

    def __init__(self, directory):
        self.directory = directory.resolve(strict=True)
        assert not (self.directory / "index.json").is_symlink()
        assert not (self.directory / "blobs").is_symlink()
        self.index = load(self.directory / "index.json")
        assert self.index["schema"] == 1 and self.index["kind"] == "native-tools-evidence"
        self.source = None
        self.used_external = set()
        self.verified = set()
        assert not (self.index["files"].keys() & self.index["external"].keys())

    def logical(self, path):
        path = Path(path)
        if self.source is not None and path.is_relative_to(self.source):
            path = Path(self.index["source_root"]) / path.relative_to(self.source)
        return str(path)

    def identity(self, path):
        name = self.logical(path)
        if name in self.index["external"]:
            self.used_external.add(name)
            item = self.index["external"][name]
        else:
            item = self.index["files"][name]
            blob = self.directory / safe_relative(item["blob"])
            assert item["blob"] == "blobs/" + item["sha256"]
            assert blob.is_file() and not blob.is_symlink()
            assert blob.stat().st_size == item["bytes"]
            if item["blob"] not in self.verified:
                data = blob.read_bytes()
                assert len(data) == item["bytes"] and sha(data) == item["sha256"], name
                self.verified.add(item["blob"])
        return {key: item[key] for key in ("sha256", "bytes")}

    def bind(self, path, expected=None):
        actual = self.identity(path)["sha256"]
        assert expected is None or actual == expected, (path, "SHA-256 differs")
        return actual

    def path(self, path):
        name = self.logical(path)
        self.identity(name)
        return self.directory / self.index["files"][name]["blob"]

    def read(self, path):
        return self.path(path).read_bytes()

    def json(self, path):
        return json.loads(self.read(path))

    def tree(self, directory):
        name = self.logical(directory)
        declared = self.index["trees"][name]
        prefix = name.rstrip("/") + "/"
        files = {path[len(prefix):]: self.identity(path) for path in self.index["files"] if path.startswith(prefix)}
        assert not any(path.startswith(prefix) for path in self.index["external"]), "retained effects cannot exclude binaries"
        assert files == declared["files"], name
        directories = declared["directories"]
        assert sorted(set(directories), key=Path) == directories and not set(directories) & files.keys()
        for path in [*files, *directories]:
            relative = safe_relative(path)
            assert all(str(parent) in directories for parent in relative.parents if str(parent) != ".")
        return {"files": files, "directories": directories}

    def verify(self):
        self.verified.clear()
        for path in self.index["files"]:
            self.identity(path)
        expected = {item["blob"] for item in self.index["files"].values()}
        actual = {str(path.relative_to(self.directory)) for path in (self.directory / "blobs").iterdir()}
        assert actual == expected
        for name, expected in self.index["tools"].items():
            path = self.directory / safe_relative(name)
            assert not path.is_symlink()
            data = path.read_bytes()
            assert {"sha256": sha(data), "bytes": len(data)} == expected

    def sources(self, destination):
        self.source = destination
        root = Path(self.index["source_root"])
        for original in self.index["files"]:
            path = Path(original)
            if path.is_relative_to(root):
                target = destination / safe_relative(str(path.relative_to(root)))
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(self.read(original))


def replay(args):
    archive = Archive(args.archive)
    output = args.output.resolve()
    assert not output.exists() and not output.is_relative_to(archive.directory)
    archive.verify()
    audit = archive.json(archive.index["audit"])
    assert audit["status"] == "passed" and audit["input_guards_passed"]
    assert archive.index["records"] == audit["records"]
    for original, expected in audit["guarded_inputs"].items():
        archive.bind(original, expected)
    assert archive.index["trees"] == audit["guarded_trees"]
    for path in archive.index["trees"]:
        archive.tree(path)
    records = [archive.json(path) for path in archive.index["records"]]
    assert 1 <= len(records) <= 2
    for path, expected in archive.index["records"].items():
        archive.bind(path, expected)
    with tempfile.TemporaryDirectory(prefix="imre-evidence-") as temporary:
        source = Path(temporary) / "source"
        archive.sources(source)
        spec = importlib.util.spec_from_file_location("native_tools_archive_audit", archive.directory / "native_tools_audit.py")
        verifier = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(verifier)
        verifier.ROOT = Path(archive.index["source_root"])
        verifier.HERE = verifier.ROOT / "benchmarks"
        verifier.bind, verifier.identity, verifier.tree = archive.bind, archive.identity, archive.tree
        verifier.read_bytes, verifier.read_json, verifier.file_hash = archive.read, archive.json, archive.bind
        catalog = SimpleNamespace(**runpy.run_path(str(source / "benchmarks/catalog_validate.py")))
        conformance = SimpleNamespace(**runpy.run_path(str(source / "benchmarks/conformance_validate.py")))

        def metadata(workload):
            manifest = archive.json(workload.manifest)
            return {"suite_revision": manifest["suite_revision"], "format": workload.format,
                    "force_summary_table": workload.force_summary_table, "test_files": len(manifest["test_files"]),
                    "test_source_bytes": sum(archive.identity(path)["bytes"] for path in manifest["test_files"]),
                    "old_diagnostics": len(archive.json(workload.old)), "new_diagnostics": len(archive.json(workload.new)),
                    "old_json_bytes": archive.identity(workload.old)["bytes"], "new_json_bytes": archive.identity(workload.new)["bytes"],
                    "manifest_sha256": archive.bind(workload.manifest), "old_sha256": archive.bind(workload.old),
                    "new_sha256": archive.bind(workload.new), "source_sha256": {path: archive.bind(path) for path in manifest["test_files"]}}

        conformance.workload_metadata = metadata
        original_commands = verifier.commands

        def commands(*arguments):
            result = original_commands(*arguments)
            return {engine: [part.replace(str(source), archive.index["source_root"]) for part in command]
                    for engine, command in result.items()}

        def expected_output(row, catalog, conformance):
            if row["tool"] == "catalog":
                value = catalog.case_from_record(row["case"])
                arguments = list(value.arguments)
                position = 0 if row["rounds"] is None else 1
                arguments[position] = str(archive.path(arguments[position]))
                value = catalog.Case(value.label, value.files, tuple(arguments), value.directories, value.hardlinks)
                expected = (catalog.oracle if row["rounds"] is None else catalog.compute_oracle)(value)
                assert expected["error"] is None
                return expected["files"][row["output"]], expected.get("stdout", "")
            paths = row["workload"]
            workload = conformance.Workload(*(archive.path(paths[name]) for name in ("manifest", "old", "new")), row["format"], True)
            contents = {path: archive.read(path).decode("utf-8") for path in archive.json(paths["manifest"])["test_files"]}
            expected = (conformance.oracle(workload, source_contents=contents) if row["rounds"] is None else
                        conformance.compute_oracle(workload, row["rounds"], source_contents=contents))
            assert expected.error is None and expected.report is not None
            return expected.report.encode(), "" if row["rounds"] is None else expected.stdout.decode()

        verifier.commands, verifier.expected_output = commands, expected_output
        modes = {}
        for record in records:
            mode = "thinlto" if record["preparation"]["thinlto"] else "normal"
            assert mode not in modes
            modes[mode] = verifier.audit_mode(record, catalog, conformance)
            assert modes[mode] == audit["modes"][mode], mode
        parity = verifier.cross_mode_input_parity(records)
        assert parity == audit["cross_mode_input_parity"]
        archive.verify()
    result = {"schema": 1, "kind": "native-tools-evidence-replay", "status": "passed",
              "measurement_revision": audit["measurement_revision"], "modes": modes,
              "cross_mode_input_parity": parity,
              "index_sha256": sha((archive.directory / "index.json").read_bytes()),
              "replay_script_sha256": sha(Path(__file__).read_bytes()),
              "retained_files_verified": len(archive.index["files"]), "directories_verified": len(archive.index["trees"]),
              "excluded_artifacts_not_rehashed": archive.index["external"],
              "licenses": archive.index["licenses"]}
    output.parent.mkdir(parents=True, exist_ok=True)
    write(output, result)
    return {"status": "passed", "modes": list(modes), "output": str(output),
            "retained_files_verified": result["retained_files_verified"],
            "excluded_artifacts_not_rehashed": len(archive.index["external"])}


def main():
    assert not sys.flags.optimize and sys.version_info >= (3, 14), "use CPython 3.14+ with assertions enabled"
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    make = modes.add_parser("package")
    make.add_argument("--audit", type=Path, required=True)
    make.add_argument("--source-binding", type=Path)
    make.add_argument("--codon-license", type=Path, required=True)
    make.add_argument("--extra", type=Path, action="append", default=[])
    make.add_argument("--output-dir", type=Path, required=True)
    check = modes.add_parser("replay")
    check.add_argument("archive", type=Path)
    check.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(package(args) if args.mode == "package" else replay(args), indent=2))


if __name__ == "__main__":
    main()
