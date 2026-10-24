"""Independently verify the completed iterator evidence archive and manifest.

No packager code is imported, and no archived script or native program executes.
The audit rehashes every member and excluded local artifact, checks immutable Git
source snapshots, and binds validation, every timing batch, preparations, and the
six optional profiles. Use --require-profile for the final milestone package.
"""

import argparse
import base64
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import subprocess
import tarfile


def check(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def forbidden(header):
    four = header[:4]
    return (four in {b"\x7fELF", b"BC\xc0\xde", b"\xde\xc0\x17\x0b", b"\0asm",
                     b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf",
                     b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",
                     b"\xca\xfe\xba\xbf", b"\xbf\xba\xfe\xca"}
            or header.startswith((b"!<arch>\n", b"!<thin>\n", b"MZ"))
            or four[2:] == b"\r\n" and (3000 <= int.from_bytes(four[:2], "little") < 4000
                                         or int.from_bytes(four[:2], "little") == 62211))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--root", type=Path, default=Path("/home/dev-user/code/oss/imre"))
    parser.add_argument("--require-profile", action="store_true")
    args = parser.parse_args()
    args.root = args.root.resolve(strict=True)
    check(not args.output.exists(), "audit output already exists")
    before = {str(path): sha(path) for path in (args.archive, args.manifest, Path(__file__))}
    manifest = json.loads(args.manifest.read_text())
    check(manifest["status"] == "passed", "package manifest is not passed")
    check(before[str(args.archive)] == manifest["archive"]["sha256"]
          and args.archive.stat().st_size == manifest["archive"]["bytes"], "archive identity differs")
    with args.archive.open("rb") as stream:
        header = stream.read(10)
    check(header[:3] == b"\x1f\x8b\x08" and header[3] == 0 and header[4:8] == b"\0" * 4,
          "gzip metadata is not normalized")
    members, jsons, patch = {}, {}, None
    total = 0
    with tarfile.open(args.archive, "r|gz") as archive:
        for member in archive:
            name = member.name
            safe = PurePosixPath(name)
            check(member.isfile() and str(safe) == name and not safe.is_absolute()
                  and ".." not in safe.parts and name not in members, f"unsafe/duplicate member: {name}")
            check(member.uid == member.gid == member.mtime == 0 and not member.uname and not member.gname
                  and member.mode in (0o644, 0o755), f"unnormalized member: {name}")
            check(name in manifest["members"], f"unmanifested member: {name}")
            keep = name.endswith(".json") and not name.startswith("source/") or name == "provenance/post-validation.patch"
            chunks, first, count, hasher = [], b"", 0, hashlib.sha256()
            stream = archive.extractfile(member)
            while chunk := stream.read(1024 * 1024):
                if not first:
                    first = chunk[:16]
                hasher.update(chunk)
                count += len(chunk)
                if keep:
                    chunks.append(chunk)
            check(not forbidden(first), f"native code/bytecode in archive: {name}")
            value = {"sha256": hasher.hexdigest(), "bytes": count, "mode": member.mode}
            check(count == member.size and value == manifest["members"][name], f"member hash/size differs: {name}")
            members[name] = value
            total += count
            if keep:
                data = b"".join(chunks)
                if name.endswith(".json"):
                    jsons[name] = json.loads(data)
                else:
                    patch = data
    check(list(members) == sorted(manifest["members"]) and len(members) == manifest["member_count"],
          "archive order or member coverage differs")
    check(not any(name.startswith("source/benchmarks/results/") or name.endswith("-evidence.tar.gz")
                  for name in members), "historical evidence nested in package")
    package = jsons["provenance/package.json"]
    check(all(manifest[key] == value for key, value in package.items()), "package provenance differs from manifest")
    exclusions = jsons["provenance/exclusions.json"]
    excluded = {value["would_be_member"]: value for value in exclusions}
    check(len(excluded) == len(exclusions) == manifest["excluded_artifact_count"], "duplicate exclusion")
    external_before = {}
    for name, value in excluded.items():
        check(name not in members, f"excluded member was archived: {name}")
        path = Path(value["path"])
        if str(path) not in external_before:
            external_before[str(path)] = sha(path)
        actual = external_before[str(path)]
        check(actual == value["sha256"] and path.stat().st_size == value["bytes"],
              f"excluded artifact identity differs: {path}")
        with path.open("rb") as stream:
            check(value["reason"] == "external tool executable" or forbidden(stream.read(16)),
                  f"non-native artifact excluded without a tool reason: {path}")

    def bound(name, expected, *, may_be_excluded=False):
        if name in members:
            actual = members[name]["sha256"]
        else:
            check(may_be_excluded and name in excluded, f"required member missing: {name}")
            actual = excluded[name]["sha256"]
        check(actual == expected, f"provenance hash mismatch: {name}")

    def find_bound(prefix, basename, expected, *, omitted=False):
        choices = excluded if omitted else members
        names = [name for name, value in choices.items() if name.startswith(prefix + "/")
                 and PurePosixPath(name).name == basename and value["sha256"] == expected]
        check(len(names) == 1, f"expected one bound artifact: {prefix}/{basename}")
        return names[0]

    validation = jsons["validation/index.json"]
    build = jsons["validation/build.json"]
    vaudit, taudit = jsons["audits/validation.json"], jsons["audits/timing.json"]
    check(all(value["status"] == "passed" for value in (validation, vaudit, taudit)), "unpassed validation/audit")
    check(validation["source_unchanged"] and validation["input_guards_passed"]
          and build["source_matches_revision"] and build["input_guards_passed"], "validation/build guards failed")
    revision, timed = package["validation_revision"], package["timing_revision"]
    check(validation["revision"] == build["revision"] == vaudit["validation_revision"] == revision,
          "validation revision mismatch")
    for name, value in (("validation/index.json", package["validation_index_sha256"]),
                        ("validation/build.json", package["validation_build_sha256"]),
                        ("audits/validation.json", package["validation_audit_sha256"]),
                        ("audits/timing.json", package["timing_audit_sha256"]),
                        ("helpers/package-evidence.py", package["packager_sha256"]),
                        ("helpers/validation-audit.py", vaudit["audit_script_sha256"]),
                        ("helpers/timing-audit.py", taudit.get("auditor_sha256", taudit.get("audit_script_sha256"))),
                        ("validation/runner.py", validation["runner_sha256"]),
                        ("validation/evidence-sha256.json", validation["evidence_manifest_sha256"])):
        bound(name, value)
    validation_dir = Path(validation["build_record"]["path"]).parent
    for job in validation["jobs"]:
        check(job["status"] == "passed" and job["exit_code"] == 0, "incomplete validation job")
        for key in ("record", "stdout", "stderr"):
            bound("validation/" + Path(job[key]).relative_to(validation_dir).as_posix(),
                  job[key + "_sha256"], may_be_excluded=key in ("stdout", "stderr"))
    check(validation["job_counts"] == {"passed": len(validation["jobs"]), "failed": 0}
          and vaudit["counts"]["passed_jobs"] == len(validation["jobs"])
          and vaudit["counts"]["rust_units"]["total"] == validation["unit_tests_passed"]
          and package["validation_counts"] == vaudit["counts"], "validation counts differ")
    for name, value in jsons["validation/evidence-sha256.json"].items():
        bound("validation/" + name, value, may_be_excluded=True)
    source = jsons["validation/source-before.json"]
    check(source == jsons["validation/source-after.json"] and source["revision"] == revision
          and source["status"] == "", "source snapshots differ")
    check({name.removeprefix("source/") for name in members if name.startswith("source/")}
          == set(source["files"]), "archived source file set differs")
    for name, value in source["files"].items():
        bound("source/" + name, value["sha256"])
        check(bool(members["source/" + name]["mode"] & 0o111) == value["executable"], "source executable bit differs")

    def git(*arguments):
        return subprocess.check_output(["git", "-C", str(args.root), *arguments])

    check(git("rev-parse", revision + "^{tree}").decode().strip() == source["tree"] == package["validation_tree"],
          "validation Git tree identity differs")
    with tarfile.open(fileobj=io.BytesIO(git("archive", "--format=tar", revision, "--", *sorted(source["files"])))) as archive:
        original = {item.name: digest(archive.extractfile(item).read()) for item in archive if item.isfile()}
    check(original == {name: value["sha256"] for name, value in source["files"].items()}, "source differs from Git commit")
    changed = git("diff", "--name-only", revision, timed).decode().splitlines()
    check(changed == package["post_validation_paths"] and set(changed) <= {
        "benchmarks/telemetry_benchmark.py", "benchmarks/reference/telemetry_comparison/README.md"},
        "unexpected post-validation source change")
    check(patch == git("diff", "--no-ext-diff", "--no-textconv", revision, timed)
          and digest(patch) == package["post_validation_patch_sha256"], "post-validation patch differs")
    for name in changed:
        bound("timing-source-overlay/" + name, digest(git("show", f"{timed}:{name}")))
    for name, value in build["source_sha256"].items():
        bound("source/" + name, value)

    preparations = {}
    for name, prepared in jsons.items():
        if name.startswith("preparation/") and name.count("/") == 1:
            label = PurePosixPath(name).stem
            check(prepared["kind"] == "telemetry-preparation" and prepared["input_guards_passed"]
                  and prepared["revision"] == timed and not prepared["dirty"]
                  and prepared["imre_build_record"] == build, "invalid preparation")
            preparations[members[name]["sha256"]] = prepared
            for key in ("llvm", "validation", "binary"):
                basename = Path(prepared["validation_path" if key == "validation" else key]).name
                find_bound("preparation/" + label, basename, prepared[key + "_sha256"], omitted=key == "binary")
            for key, path in prepared["source_and_tool_paths"].items():
                path = Path(path)
                if args.root in path.parents:
                    relative = path.relative_to(args.root).as_posix()
                    prefix = "timing-source-overlay/" if relative in changed else "source/"
                    bound(prefix + relative, prepared["source_and_tool_sha256"][key])
            mode = "thinlto" if prepared["thinlto"] else "normal"
            reference = validation["references"]["telemetry-" + mode]
            name = "validation/" + Path(reference["build_record"]).relative_to(validation_dir).as_posix()
            bound(name, reference["sha256"])
            check(jsons[name] == prepared["rust_build_record"], "Rust reference provenance differs")
    status_runs = []
    audited_statuses = {entry["sha256"]: entry for entry in taudit["statuses"]}
    check(len(audited_statuses) == len(taudit["statuses"])
          and set(audited_statuses) == set(package["timing_status_sha256"].values()),
          "independent timing audit status coverage differs")
    for label, expected in package["timing_status_sha256"].items():
        name = f"timing/status/{label}.json"
        bound(name, expected)
        status = jsons[name]
        audited = audited_statuses[expected]
        check(status["status"] == "passed" and not status["exploratory"]
              and status["input_guards_passed"] and status["source_unchanged"]
              and status["timing_revision"] == timed and status["validation_revision"] == revision,
              "invalid timing status")
        check(audited["validation_revision"] == revision and audited["timing_revision"] == timed
              and audited["schedule"] == status["schedule"]
              and {(entry["record"], entry["mode"], entry["record_sha256"]) for entry in audited["records"]}
              == {(entry["output"], entry["mode"], entry["output_sha256"]) for entry in status["runs"]},
              "independent timing audit record bindings differ")
        check(status["post_validation_changes"]["patch"].encode() == patch
              and status["post_validation_changes"]["patch_sha256"] == digest(patch), "timing patch differs")
        for run in status["runs"]:
            check(run["status"] == "passed" and run["exit_code"] == 0, "unfinished timing run")
            status_runs.append((run["output_sha256"], run["mode"]))
    recorded_runs, raw_files = [], 0
    for label, identity in package["timing_records"].items():
        name = f"timing/records/{label}.json"
        bound(name, identity["record_sha256"])
        record = jsons[name]
        check(record["input_guards_passed"] and not record["dirty"] and record["revision"] == timed,
              "invalid timing record")
        check(record["preparation"] == preparations[record["preparation_record_sha256"]], "timed preparation differs")
        mode = "thinlto" if record["thinlto"] else "normal"
        check(mode == identity["mode"], "timing runtime mode differs")
        recorded_runs.append((identity["record_sha256"], mode))
        for workload in [record["startup_control"], *record["workloads"]]:
            for engine in ("cpython", "imre", "rust"):
                for filename, expected in workload["expected_files"].items():
                    data = base64.b64decode(expected["base64"], validate=True)
                    check(digest(data) == expected["sha256"] and len(data) == expected["bytes"], "encoded expected file differs")
                    bound(f"timing/artifacts/{label}/{workload['label']}/{engine}/{filename}", expected["sha256"])
                    raw_files += 1
    check(len(set(recorded_runs)) == len(recorded_runs) and sorted(recorded_runs) == sorted(status_runs)
          and {mode for _, mode in recorded_runs} == {"normal", "thinlto"}, "timing batches lack one-to-one coverage")

    profiles = []
    for name, summary in jsons.items():
        if name.endswith("/summary.json") and "profiles" in summary and summary.get("label") == "alert-quiet-4096":
            prefix = name.removesuffix("/summary.json")
            check(summary["status"] == "passed" and summary["complete"] and summary["input_guards_passed"]
                  and summary["timing_revision"] == timed and summary["validation_revision"] == revision,
                  "incomplete profile")
            check(summary["benchmark_record_sha256"] in {value for value, _ in recorded_runs}
                  and summary["preparation_record_sha256"] in preparations
                  and summary["timing_status_sha256"] in package["timing_status_sha256"].values(),
                  "profile does not bind the archived timing/preparation")
            prepared = preparations[summary["preparation_record_sha256"]]
            for basename, expected in (("benchmark-record.json", summary["benchmark_record_sha256"]),
                                       ("preparation.json", summary["preparation_record_sha256"]),
                                       ("timing-status.json", summary["timing_status_sha256"]),
                                       ("native.ll", prepared["llvm_sha256"]),
                                       ("imre-build-record.json", package["validation_build_sha256"]),
                                       ("rust-build-record.json", prepared["source_and_tool_sha256"]["rust_build_record"])):
                bound(prefix + "/" + basename, expected)
            for relative, expected in summary["snapshot_sha256"].items():
                bound(prefix + "/" + relative, expected)
            expected_runs = {(engine, tool, repeats) for engine in ("imre", "rust")
                             for tool, repeats in (("callgrind", 128), ("dhat", 128), ("dhat", 256))}
            check({(item["engine"], item["tool"], item["repetitions"]) for item in summary["profiles"]}
                  == expected_runs and len(summary["profiles"]) == 6, "profile run matrix differs")
            for relative, value in jsons[prefix + "/manifest.json"].items():
                bound(prefix + "/" + relative, value)
            for item in summary["profiles"]:
                check(item["status"] == "validated" and item["returncode"] == 0, "unvalidated profile run")
                expected = prepared["binary_sha256"] if item["engine"] == "imre" else prepared["source_and_tool_sha256"]["rust"]
                check(item["binary_sha256"] == expected, "profile used another native binary")
                directory = f"{prefix}/{item['engine']}-{item['tool']}-{item['repetitions']}"
                for relative, value in item["files"].items():
                    bound(directory + "/" + relative, value)
            profiles.append(prefix)
    check(not args.require_profile or len(profiles) == 1, "expected one complete telemetry profile")
    profile_audits = [value for name, value in jsons.items() if name.endswith("profile-audit.json")]
    for audit in profile_audits:
        check(audit["status"] == "passed" and audit["timing_revision"] == timed
              and any(members[prefix + "/summary.json"]["sha256"] == audit["profile_summary_sha256"]
                      and members[prefix + "/manifest.json"]["sha256"] == audit["profile_manifest_sha256"]
                      for prefix in profiles), "profile independent audit binding differs")
        check(any(name.endswith(".py") and value["sha256"] == audit["audit_script_sha256"]
                  for name, value in members.items()), "profile auditor script missing")
    check(not args.require_profile or len(profile_audits) == 1, "profile independent audit missing")
    for path, expected in {**before, **external_before}.items():
        check(sha(path) == expected, f"audit input changed: {path}")
    result = {"schema": 1, "status": "passed", "archive_sha256": before[str(args.archive)],
              "manifest_sha256": before[str(args.manifest)], "audit_script_sha256": before[str(Path(__file__))],
              "validation_revision": revision, "timing_revision": timed,
              "members_verified": len(members), "uncompressed_bytes_verified": total,
              "exclusions_verified": len(exclusions), "excluded_local_files_verified": len(external_before),
              "source_files_verified_against_git": len(original), "validation_jobs": len(validation["jobs"]),
              "timing_statuses": len(package["timing_status_sha256"]), "timing_records": len(recorded_runs),
              "timing_raw_output_files_verified": raw_files, "profile_trees": profiles,
              "limits": "Archive/provenance integrity audit; no native programs, archived scripts, builds, or measurements executed. Detailed semantic/timing/profile audits remain separately retained evidence."}
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
