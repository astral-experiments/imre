"""Validate compatibility receipts and render both public scorecard views."""
from __future__ import annotations

import argparse
import base64
import binascii
import csv
import hashlib
import json
import os
import re
from collections import Counter
from functools import lru_cache
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CANONICAL_COHORT = json.loads((HERE / "cohort.json").read_text())
DATA_ROOT = HERE
COHORT = CANONICAL_COHORT
SCHEMA = "imre.compatibility.v1"
OUTCOMES = {"pass", "fail", "error", "skip", "timeout", "not_run", "omitted"}
TIERS = ("stock", "O0", "O3")
DIMENSIONS = ("exit_code", "stdout", "stderr", "effects")
HEX40 = re.compile(r"[0-9a-f]{40}\Z")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
RESULT = re.compile(r"^([^\s]+) \(([^)]+)\) \.\.\. (ok|FAIL|ERROR|skipped .+|expected failure|unexpected success)$")
START = "<!-- compatibility-scorecard:start -->"
END = "<!-- compatibility-scorecard:end -->"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def select_snapshot(revision):
    """Select receipt data without changing repository or frozen-policy roots."""
    global DATA_ROOT, COHORT
    require(isinstance(revision, str) and bool(HEX40.fullmatch(revision)), "snapshot must be a lowercase 40-hex revision")
    root = (HERE / "revisions" / revision).resolve()
    require(root.parent == (HERE / "revisions").resolve() and root.is_dir(), "snapshot directory does not exist")
    path = root / "cohort.json"
    require(path.is_file() and path.resolve().is_relative_to(root), "snapshot cohort is missing")
    cohort = json.loads(path.read_text())
    expected = {**CANONICAL_COHORT, "imre_revision": revision}
    require(cohort == expected, "snapshot cohort must preserve the canonical cohort and match its directory revision")
    DATA_ROOT, COHORT = root, cohort


def is_snapshot():
    return DATA_ROOT != HERE


def repository_file(locator, root=ROOT):
    require(isinstance(locator, str) and bool(locator), "missing repository-local evidence path")
    path = (ROOT / locator).resolve()
    require(path.is_relative_to(root.resolve()) and path.is_file(), f"evidence outside allowed root or missing: {locator}")
    return path


def families():
    with (ROOT / COHORT["historical_families"]).open(newline="") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    result = {r["family"]: {"originals": int(r["originals"]), "historical": int(r["paired"]), "cohort": "frozen"} for r in rows}
    require(len(result) == len(rows), "duplicate historical family")
    for r in COHORT["supplemental_families"]:
        require(r["family"] not in result, "duplicate supplemental family")
        result[r["family"]] = {"originals": r["originals"], "historical": None, "cohort": "supplemental"}
    return result


def raw_observation(obj, name, check, required=False):
    keys = [key for key in (name, name + "_base64", name + "_path") if key in obj]
    digest_key = "effects_sha256" if name == "effects_manifest" else name + "_sha256"
    digest = obj.get(digest_key)
    if not keys and digest is None and not required: return None
    check(len(keys) == 1 and isinstance(digest, str) and bool(HEX64.fullmatch(digest)), f"{name}: need one raw representation and SHA-256")
    key = keys[0]; value = obj[key]
    check(isinstance(value, str), f"{key}: expected string")
    if key == name: data = value.encode("utf-8")
    elif key.endswith("_base64"):
        try: data = base64.b64decode(value, validate=True)
        except (ValueError, binascii.Error): check(False, f"{key}: invalid base64")
    else:
        path = (ROOT / value).resolve()
        check(path.is_relative_to(ROOT.resolve()) and path.is_file(), f"{key}: missing repository-local raw file")
        data = path.read_bytes()
    check(hashlib.sha256(data).hexdigest() == digest, f"{name}: digest inconsistent with raw bytes")
    if name == "effects_manifest":
        try: manifest = json.loads(data)
        except (ValueError, UnicodeError): check(False, "effects_manifest: expected raw JSON object")
        check(isinstance(manifest, dict), "effects_manifest: expected raw JSON object")
    return data


def validate_observations(obj, check, required=False):
    if required: check(type(obj.get("exit_code")) is int, "missing exit code")
    for name in ("stdout", "stderr", "effects_manifest"):
        raw_observation(obj, name, check, required)


def valid_preimages(preimages, check):
    check(isinstance(preimages, dict) and all(isinstance(k, str) and k and not k.startswith("/") and ".." not in Path(k).parts and (v is None or isinstance(v, str) and HEX64.fullmatch(v)) for k, v in preimages.items()), "invalid fixture preimages")


def validate_frozen_anchor(anchor):
    label = anchor.get("id", "<missing id>")
    def check(c, msg): require(c, f"frozen {label}: {msg}")
    check(anchor.get("schema") == "imre.compatibility.frozen-script.v1" and isinstance(label, str) and bool(label), "invalid schema or id")
    source = anchor.get("source", {})
    check(all(isinstance(source.get(k), str) and source[k] for k in ("repo", "commit", "path", "sha256")) and bool(HEX40.fullmatch(source["commit"])) and bool(HEX64.fullmatch(source["sha256"])), "invalid source")
    evidence = anchor.get("evidence", {})
    check(isinstance(evidence, dict) and isinstance(evidence.get("locator"), str) and bool(evidence["locator"]), "missing frozen evidence locator")
    data = raw_observation(evidence, "raw", check, True)
    try: frozen = json.loads(data)
    except (ValueError, UnicodeError): check(False, "frozen evidence must be JSON")
    check(isinstance(frozen, dict) and isinstance(frozen.get("fixture_scope"), str) and bool(frozen["fixture_scope"]), "missing frozen fixture scope")
    check(frozen.get("id") == label and frozen.get("source") == source and frozen.get("normalized_invocation") == anchor.get("normalized_invocation") and isinstance(anchor.get("normalized_invocation"), dict), "frozen evidence does not bind the original command")
    valid_preimages(frozen.get("preimages"), check)
    reference = frozen.get("reference")
    check(isinstance(reference, dict), "missing frozen reference")
    validate_observations(reference, check, True)
    return frozen


def load_frozen_scripts():
    path = HERE / "frozen" / "scripts.jsonl"
    anchors = {}
    if path.exists():
        for line in path.read_text().splitlines():
            anchor = json.loads(line); validate_frozen_anchor(anchor)
            require(anchor["id"] not in anchors, f"duplicate frozen script anchor: {anchor['id']}")
            anchors[anchor["id"]] = anchor
    return anchors



@lru_cache(maxsize=None)
def canonical_originals(kind):
    originals = {}
    for path in sorted((HERE / ("stdlib" if kind == "stdlib" else "scripts")).glob("*.jsonl")):
        for line in path.read_text().splitlines():
            row = json.loads(line)
            key = (row.get("family"), row["id"], row.get("occurrence")) if kind == "stdlib" else row["id"]
            require(key not in originals, f"duplicate canonical original: {key}")
            originals[key] = row
    return originals


def canonical_original(r, kind):
    key = (r.get("family"), r["id"], r.get("occurrence")) if kind == "stdlib" else r["id"]
    original = canonical_originals(kind).get(key)
    if original is None and kind == "stdlib" and r.get("occurrence") == 1:
        # Some frozen originals have no canonical receipt. A distinct ID in a
        # pinned reference inventory and its vendored manifest can still anchor
        # a fresh stock run; a dict inventory cannot establish repeat occurrences.
        suite = HERE.parent / "cpython_stdlib/full" / r["family"]
        reference, manifest = suite / "reference-linux.json", suite / "UPSTREAM.json"
        if reference.is_file() and manifest.is_file():
            inventory, upstream = json.loads(reference.read_text()), json.loads(manifest.read_text())
            source = r["source"]; module = source_module(source["path"])
            if isinstance(inventory.get("reference"), dict) and r["id"] in inventory["reference"] and r["id"].startswith(module + ".") and inventory.get("cpython_commit") == COHORT["cpython_commit"] and upstream.get("commit") == COHORT["cpython_commit"] and upstream.get("files", {}).get(source["path"]) == source.get("git_blob"):
                vendored = suite / "upstream" / source["path"]
                if vendored.is_file():
                    stat = vendored.stat()
                    if source_digests(vendored, stat.st_mtime_ns, stat.st_size) == (source.get("sha256"), source.get("git_blob")):
                        original = {"source": source, "runs": {}}
    require(original is not None and original["source"] == r["source"], f"{r['id']}: original ID, occurrence or source differs from canonical inventory")
    if kind == "script":
        require(original.get("normalized_invocation") == r.get("normalized_invocation"), f"{r['id']}: original command differs from canonical inventory")
    return original


def referenced_json(ref):
    require(isinstance(ref, dict) and isinstance(ref.get("sha256"), str) and bool(HEX64.fullmatch(ref["sha256"])), "missing evidence reference digest")
    path = repository_file(ref.get("path"), DATA_ROOT)
    data = path.read_bytes()
    require(hashlib.sha256(data).hexdigest() == ref["sha256"], f"evidence digest differs: {path}")
    value = json.loads(data)
    require(isinstance(value, dict), "referenced evidence must be a JSON object")
    return value


def validate_toolchain(toolchain):
    require(isinstance(toolchain, dict) and all(toolchain.get(k) == COHORT["imre_revision"] for k in ("compiler_revision", "runtime_revision")), "compiler/runtime revision differs from snapshot")
    require(all(isinstance(toolchain.get(k), str) and HEX64.fullmatch(toolchain[k]) for k in ("compiler_sha256", "runtime_sha256")), "missing compiler/runtime identities")


def validate_launcher_control(r):
    control = referenced_json(r["launcher_control"])
    launcher = r.get("launcher_id")
    require(isinstance(launcher, str) and launcher and control.get("launcher_id") == launcher and control.get("imre_revision") == COHORT["imre_revision"], "launcher control revision or launcher differs")
    toolchain = r["runs"]["O0"].get("toolchain")
    require(control.get("schema") == "imre.compatibility.launcher-control.v1" and control.get("toolchain") == toolchain, "launcher control schema or compiler/runtime identities differ")
    poison = control.get("poison", {})
    require(isinstance(poison, dict) and poison.get("imported_source") == "status: int = True" and isinstance(poison.get("imported_module"), str) and poison["imported_module"], "launcher control must poison an imported module")
    argv = poison.get("argv"); original_build = r["runs"]["O0"].get("build_argv", [])
    require(isinstance(argv, list) and len(argv) >= 3 and argv[:3] == original_build[:3] and "--no-cache" in argv and option(argv, "--opt-level") == "0" and option(argv, "--runtime") == option(original_build, "--runtime") and poison.get("launcher_id") == launcher, "control must build the actual launcher at O0 with the matched runtime and no cache")
    source = repository_file(poison.get("source_path"), DATA_ROOT)
    require(bool(HEX64.fullmatch(poison.get("source_sha256", ""))) and hashlib.sha256(source.read_bytes()).hexdigest() == poison["source_sha256"] and source.read_bytes() == b"status: int = True\n" and source.stem == poison["imported_module"], "missing or altered imported poison source")
    positive = repository_file(poison.get("positive_launcher_path"), DATA_ROOT); poisoned = repository_file(poison.get("poisoned_launcher_path"), DATA_ROOT)
    injection = f"import {source.stem}\n".encode(); lines = poisoned.read_bytes().splitlines(keepends=True)
    require(hashlib.sha256(positive.read_bytes()).hexdigest() == poison.get("positive_launcher_sha256") and hashlib.sha256(poisoned.read_bytes()).hexdigest() == poison.get("poisoned_launcher_sha256") and lines.count(injection) == 1 and b"".join(line for line in lines if line != injection) == positive.read_bytes(), "launcher hashes or explicit poison import differ")
    for key in ("stdout_path", "stderr_path", "effects_manifest_path"):
        if key in poison: repository_file(poison[key], DATA_ROOT)
    validate_observations(poison, require, False)
    stderr = raw_observation(poison, "stderr", require, True).decode("utf-8", errors="replace").lower()
    passed = type(poison.get("exit_code")) is int and poison["exit_code"] != 0 and poison.get("timed_out", False) is False and poison.get("image_created") is False and bool(re.search(r"expected\s+.*?int.*?(?:got|found)\s+.*?bool", stderr, re.S))
    if not passed: return False
    checks = control.get("saved_checks", {})
    require(isinstance(checks, dict), "missing saved-image checks")
    for tier in ("O0", "O3"):
        witness = referenced_json(checks.get(tier))
        run = r["runs"][tier]
        require(witness.get("imre_revision") == COHORT["imre_revision"] and witness.get("launcher_id") == launcher and witness.get("tier") == tier, "saved-image check does not match launcher, tier or revision")
        require(bool(HEX64.fullmatch(run.get("binary_sha256", ""))) and witness.get("binary_sha256") == run["binary_sha256"], "saved-image binary identity differs")
        require(witness.get("source_withdrawn") is True and witness.get("child_exec_observed") is False and witness.get("libpython_loaded") is False, "saved-image source-withdrawal, child-exec or libpython check failed")
        ref = witness.get("outcome_evidence", {}); path = repository_file(ref.get("path"), DATA_ROOT); stat = path.stat()
        require(source_digests(path, stat.st_mtime_ns, stat.st_size)[0] == ref.get("sha256") and type(ref.get("line")) is int and ref["line"] > 0, "saved-image outcome evidence digest or line differs")
        if r["kind"] == "stdlib":
            outcome = raw_evidence_index(path, source_module(r["source"]["path"]), stat.st_mtime_ns, stat.st_size).get(ref["line"])
            require(outcome == (r["id"], r["occurrence"], run["outcome"]), "saved-image exact original outcome differs")
        else:
            lines = path.read_text().splitlines(); observed = json.loads(lines[ref["line"]-1]) if ref["line"] <= len(lines) else {}
            require(observed.get("id") == r["id"] and observed.get("tier") == tier and observed.get("outcome") == run["outcome"], "saved-image exact original outcome differs")
    return True


def accepted_policy_locator(r):
    policy = HERE.parent / "cpython_stdlib/full/argparse/accepted-omissions-linux.tsv"
    if r.get("family") != "argparse": return None
    with policy.open(newline="") as f:
        for line, row in enumerate(csv.DictReader(f, delimiter="\t"), 2):
            if row["full_id"] == r["id"] and int(row["occurrence"]) == r["occurrence"] and row["decision"] == "accepted":
                return f"{policy.relative_to(ROOT).as_posix()}#L{line}"
    return None



def checked_file(path, digest=None):
    path = repository_file(path.relative_to(ROOT).as_posix(), DATA_ROOT)
    if digest is not None:
        stat = path.stat()
        require(isinstance(digest, str) and bool(HEX64.fullmatch(digest)) and source_digests(path, stat.st_mtime_ns, stat.st_size)[0] == digest, f"raw artifact digest differs: {path}")
    return path


def artifact_json(path):
    value = json.loads(checked_file(path).read_text())
    require(isinstance(value, dict), f"invalid artifact JSON: {path}")
    return value


def runtime_argument(argv):
    require(isinstance(argv, list) and len(argv) >= 3 and argv[1] == "build" and "--runtime" in argv and len(argv) > argv.index("--runtime") + 1, "missing matched native build invocation")
    return argv[argv.index("--runtime") + 1]


def option(argv, name):
    require(isinstance(argv, list) and argv.count(name) == 1 and argv.index(name) + 1 < len(argv), f"missing or repeated {name}")
    return argv[argv.index(name) + 1]


def file_ref(path):
    path = checked_file(path); stat = path.stat()
    return {"path": path.relative_to(ROOT).as_posix(), "sha256": source_digests(path, stat.st_mtime_ns, stat.st_size)[0]}


def raw_sequence(path, module):
    path = checked_file(path); stat = path.stat()
    return list(raw_evidence_index(path, module, stat.st_mtime_ns, stat.st_size).values())


def comparable_build(argv, copied=False):
    # The output image always differs. A copied FILE graph may also move the
    # entry and module root, whose relative relationship is checked separately.
    value = list(argv); value[value.index("-o") + 1] = "<image>"
    if copied:
        value[2] = "<launcher>"; value[value.index("--module-root") + 1] = "<root>"
    return value


@lru_cache(maxsize=None)
def adapt_family_replay(root, family, revision, source_path, source_sha, source_blob):
    """Adapt the published family manifests into explicit, verified references.

    Rows remain byte-for-byte unchanged. The adapter accepts only the two
    manifest layouts with archived launcher/control/withdrawal evidence.
    """
    directory = root / "evidence" / family
    build_path, run_path = directory / "build.json", directory / "run.json"
    build, run = artifact_json(build_path), artifact_json(run_path)
    require(build.get("source_revision") == revision and (build.get("clean_worktree") is True or build.get("source_clean") is True) and run.get("imre_revision") == revision, "family build/run revision or clean source differs from snapshot")
    if "exit_code" in build: require(build["exit_code"] == 0, "family toolchain build failed")
    if "source_blob" in run: require(run["source_blob"] == source_blob, "family source blob differs")
    artifacts, tiers = build.get("artifacts", {}), run.get("runs", {})
    require(isinstance(artifacts, dict) and isinstance(tiers, dict), "missing family artifacts or runs")
    identity = None; launcher = None
    for tier in ("O0", "O3"):
        meta = tiers.get(tier, {}); argv = meta.get("build_argv"); runtime = runtime_argument(argv)
        require(option(argv, "--opt-level") == tier[1:] and "--no-cache" in argv and meta.get("build_exit_code") == 0 and meta.get("exit_code") == 0 and meta.get("timed_out") is False, f"{tier} family build/run differs")
        current = (argv[0], artifacts.get(argv[0]), runtime, artifacts.get(runtime))
        require(all(isinstance(x, str) and HEX64.fullmatch(x) for x in (current[1], current[3])) and (identity is None or identity == current), "family compiler/runtime artifacts differ between levels")
        require(launcher is None or launcher == argv[2], "family launcher differs between levels")
        image = meta.get("image", meta.get("argv", [None])[0]); digest = meta.get("image_sha256", meta.get("executable_sha256"))
        require(option(argv, "-o") == image and image in meta.get("argv", []) and isinstance(digest, str) and bool(HEX64.fullmatch(digest)), f"{tier} saved image identity or invocation differs")
        identity, launcher = current, argv[2]
    locator = run.get("architecture_control")
    if locator is None and "launcher_control_passed" in run:
        locator = (directory / "control.json").relative_to(ROOT).as_posix()
    require(isinstance(locator, str), "family launcher has no referenced control")
    control_path = repository_file(locator, root); control = artifact_json(control_path)
    copied = "graph_build" in control
    graph = control.get("graph_build", control); argv = graph.get("argv"); runtime = runtime_argument(argv)
    require(argv[0] == identity[0] and runtime == identity[2] and option(argv, "--runtime") == identity[2] and option(argv, "--opt-level") == "0" and "--no-cache" in argv and control.get("imre_revision", revision) == revision, "launcher control did not use O0 and the exact pinned compiler/runtime")
    require(comparable_build(argv, copied) == comparable_build(tiers["O0"]["build_argv"], copied), "poisoned build flags differ from the positive O0 launcher")
    poison_name = Path(control.get("control_module", "")).name or "fnmatch_poison.py"
    poison = checked_file(control_path.parent / poison_name, control.get("control_module_sha256", control.get("poison_sha256")))
    require(poison.read_bytes() == b"status: int = True\n", "control source is not the imported bad-type assignment")
    if copied:
        require(control.get("positive_launcher_sha256") == source_sha and control.get("saved_images_copied_before_runner_cleanup") is True, "control does not bind the original positive launcher")
        original = ROOT / "benchmarks/cpython_stdlib/full" / family / "upstream" / source_path
        require(original.is_file() and hashlib.sha256(original.read_bytes()).hexdigest() == source_sha, "positive FILE source is unavailable or altered")
        poisoned = checked_file(control_path.parent / "poisoned_test_fnmatch.py", control.get("poisoned_launcher_sha256"))
        import_line = f"import {poison.stem}\n".encode(); lines = original.read_bytes().splitlines(keepends=True)
        require(lines and poisoned.read_bytes() == lines[0] + import_line + b"".join(lines[1:]) and control.get("launcher_change") == f"insert import {poison.stem} after the module docstring; otherwise unchanged original launcher", "poisoned FILE copy or explicit import injection differs")
        positive_root, poisoned_root = option(tiers["O0"]["build_argv"], "--module-root"), option(argv, "--module-root")
        require(Path(launcher).is_relative_to(positive_root) and Path(argv[2]).is_relative_to(poisoned_root) and Path(launcher).relative_to(positive_root) == Path(argv[2]).relative_to(poisoned_root), "poisoned FILE copy has a different module entry")
        stderr = checked_file(control_path.parent / "O0.poison-build.stderr", graph.get("stderr_sha256")).read_text().lower(); image_created = graph.get("executable_exists")
    else:
        positive = checked_file(control_path.parent / Path(launcher).name, control.get("positive_launcher_sha256"))
        injected = f"import {poison.stem}\n".encode() + positive.read_bytes()
        require(control.get("positive_launcher") == launcher == argv[2] and control.get("launcher_restored_by_hash") is True and control.get("injection") == f"import {poison.stem} prepended to the actual positive launcher, then restored by hash" and hashlib.sha256(injected).hexdigest() == control.get("control_launcher_sha256"), "control does not bind the positive launcher and import injection")
        stderr = checked_file(control_path.parent / "control.O0.build.stderr").read_text().lower(); image_created = control.get("image_existed")
    passed = type(graph.get("exit_code")) is int and graph["exit_code"] != 0 and graph.get("timed_out", False) is False and image_created is False and bool(re.search(r"expected\s+.*?int.*?(?:got|found)\s+.*?bool", stderr, re.S))
    stock = tiers.get("stock")
    if stock is None:
        stock = artifact_json(directory / "stock.json")
        require(stock.get("source") == {"repo": "https://github.com/python/cpython", "commit": COHORT["cpython_commit"], "path": source_path, "sha256": source_sha, "git_blob": source_blob}, "stock manifest source differs from the canonical original")
    require(isinstance(stock, dict) and stock.get("exit_code") == 0 and isinstance(stock.get("argv"), list), "missing fresh stock launcher manifest")
    references = {"stock": stock, "toolchain_evidence": file_ref(build_path), "run_evidence": file_ref(run_path), "launcher_control": file_ref(control_path), "toolchain": {"compiler_revision": revision, "runtime_revision": revision, "compiler_sha256": identity[1], "runtime_sha256": identity[3]}, "saved_checks": {}}
    if not passed: return run, references, False
    module = source_module(source_path)
    withdrawal = control.get("withdrawal")
    if copied:
        require(isinstance(withdrawal, dict) and withdrawal.get("source_absent_during_runs") is True and withdrawal.get("matching_bytecode_absent") is True and withdrawal.get("source_sha256_before") == source_sha == withdrawal.get("source_sha256_restored"), "source withdrawal did not establish the original source")
        for tier in ("O0", "O3"):
            witness = withdrawal.get("runs", {}).get(tier, {}); meta = tiers[tier]; saved = meta.get("saved_image")
            require(witness.get("exit_code") == 0 and witness.get("image_sha256") == meta.get("executable_sha256") and witness.get("image_matches_saved_baseline_hash") is True and saved in witness.get("argv", []) and witness.get("exact_id_occurrence_outcome_sequence_matches_baseline") is True and witness.get("additional_exec_calls") == [] and witness.get("linked_libpython") is False and witness.get("ldd_exit_code") == 0, f"{tier} saved-image check failed")
            trace_path = checked_file(control_path.parent / f"{tier}.withdrawn.execve", witness.get("trace_sha256")); trace = trace_path.read_text()
            calls = [line for line in trace.splitlines() if re.search(r"execve(?:at)?\(", line)]
            require(len(calls) == 1 and re.search(r"= 0(?:\s|$)", calls[0]) and calls == witness.get("exec_calls") and f'"{saved}"' in calls[0], f"{tier} child exec observed or saved image differs")
            require("libpython" not in checked_file(control_path.parent / f"{tier}.ldd.stdout").read_text().lower(), f"{tier} linked libpython")
            outcomes = checked_file(control_path.parent / f"{tier}.withdrawn.stderr", witness.get("stderr_sha256")); baseline = directory / f"{tier}.stderr"
            sequence = raw_sequence(outcomes, module)
            require(sequence and sequence == raw_sequence(baseline, module) and len(sequence) == witness.get("completed_originals"), f"{tier} source-withdrawn outcomes differ")
            references["saved_checks"][tier] = {"binary_sha256": witness["image_sha256"], "outcomes": file_ref(outcomes), "trace": file_ref(trace_path)}
    else:
        withdrawal_path = directory / "withdrawal.json"; withdrawal = artifact_json(withdrawal_path)
        require(withdrawal.get("performed") is True and withdrawal.get("source_absent") is True and withdrawal.get("restored_by_hash") is True and withdrawal.get("source_sha256_before") == source_sha == withdrawal.get("source_sha256_after") and set(withdrawal.get("runs", [])) == {"O0", "O3"}, "source withdrawal did not establish the original source")
        for tier in ("O0", "O3"):
            meta = tiers[tier]; trace_path = checked_file(directory / meta.get("raw_trace", ""), meta.get("trace_sha256")); trace = trace_path.read_text()
            successful = [line for line in trace.splitlines() if re.search(r"execve(?:at)?\(", line)]
            require(len(successful) == 1 and re.search(r"= 0(?:\s|$)", successful[0]) and meta.get("source_absent_during_run") is True and meta.get("child_exec_denied") is True and meta.get("successful_execs") == len(successful) == 1 and f'"{meta["image"]}"' in successful[0] and meta.get("no_libpython") is True and meta.get("libpython_open_lines") == 0 and "libpython" not in trace.lower() and "libpython" not in checked_file(directory / f"{tier}.ldd").read_text().lower(), f"{tier} saved-image delegation checks failed")
            outcomes = directory / f"{tier}.stderr"; sequence = raw_sequence(outcomes, module)
            require(sequence and sequence == raw_sequence(directory / "stock.stderr", module) and len(sequence) == meta.get("completed_originals"), f"{tier} source-withdrawn outcomes differ")
            references["saved_checks"][tier] = {"binary_sha256": meta["image_sha256"], "withdrawal": file_ref(withdrawal_path), "outcomes": file_ref(outcomes), "trace": file_ref(trace_path)}
    return run, references, True


def validate_family_artifact_record(r, eligible, check):
    source = r["source"]
    manifest, references, passed = adapt_family_replay(DATA_ROOT, r["family"], COHORT["imre_revision"], source["path"], source["sha256"], source["git_blob"])
    validate_toolchain(references["toolchain"])
    directory = DATA_ROOT / "evidence" / r["family"]
    stock_run = r["runs"]["stock"]; stock_path = repository_file(stock_run["evidence"].split("#L", 1)[0])
    if stock_path.is_relative_to(DATA_ROOT.resolve()):
        check(stock_run.get("argv") == references["stock"].get("argv") and stock_run.get("environment") == references["stock"].get("environment") and stock_path in {directory / "stock.results", directory / "stock.stderr"}, "stock receipt differs from the fresh stock launcher")
    for tier in ("O0", "O3"):
        observed = r["runs"][tier]; meta = manifest["runs"][tier]
        check(observed.get("build_argv") == meta.get("build_argv") and observed.get("argv") == meta.get("argv"), f"{tier} receipt does not bind the shared launcher invocation")
        for key in ("evidence", "raw_evidence"):
            if observed.get(key):
                path = repository_file(observed[key].split("#L", 1)[0], DATA_ROOT)
                check(path in {directory / f"{tier}.results", directory / f"{tier}.stderr"}, f"{tier} receipt evidence differs from the launcher run")
    check(type(r.get("paired")) is bool, "paired must be boolean")
    if r["paired"]: check(eligible and passed and set(references["saved_checks"]) == {"O0", "O3"}, "credited snapshot pair requires an eligible triple and passing launcher control")
    elif eligible: check(isinstance(r.get("credit_reason"), str) and bool(r["credit_reason"]), "uncredited observed intersection needs a credit reason")
    check(passed or not r.get("omission_reason"), "failed launcher control cannot be an accepted policy omission")


def validate_snapshot_record(r, kind, eligible, check):
    canonical_original(r, kind)
    omission = r.get("omission_reason")
    if omission:
        check(kind == "stdlib" and r.get("policy_omission") == accepted_policy_locator(r) and r.get("policy_omission") is not None, "snapshot omission is not an accepted canonical policy omission")
    if kind == "stdlib" and not r.get("toolchain_evidence") and (DATA_ROOT / "evidence" / r["family"] / "run.json").is_file():
        validate_family_artifact_record(r, eligible, check)
        return
    native = [r["runs"][tier] for tier in ("O0", "O3")]
    identities = []
    for tier, run in zip(("O0", "O3"), native):
        if run["outcome"] not in {"not_run", "omitted"}:
            validate_toolchain(run.get("toolchain")); identities.append(run["toolchain"])
            argv = run.get("build_argv")
            check(isinstance(argv, list) and len(argv) >= 3 and argv[1] == "build" and "--opt-level" in argv and argv[argv.index("--opt-level") + 1:argv.index("--opt-level") + 2] == [tier[1:]] and "--runtime" in argv and len(argv) > argv.index("--runtime") + 1, f"{tier} missing exact native build invocation")
            for key in ("stdout_path", "stderr_path", "effects_manifest_path"):
                if key in run: repository_file(run[key], DATA_ROOT)
            check(run.get("launcher_id") == r.get("launcher_id") and bool(r.get("launcher_id")), f"{tier} launcher identity differs")
        if run.get("evidence"):
            locator = run["evidence"].split("#L", 1)[0]
            repository_file(locator, DATA_ROOT)
    check(not identities or all(x == identities[0] for x in identities), "O0/O3 compiler/runtime identities differ")
    if identities:
        build = referenced_json(r.get("toolchain_evidence"))
        check(build.get("source_revision") == COHORT["imre_revision"] and build.get("clean_source") is True and build.get("toolchain") == identities[0] and isinstance(build.get("build_argv"), list) and bool(build["build_argv"]), "toolchain build receipt does not establish the matched source and artifacts")
        artifacts = build.get("artifacts", {})
        for run in native:
            if run["outcome"] in {"not_run", "omitted"}: continue
            argv = run["build_argv"]; compiler, runtime = argv[0], argv[argv.index("--runtime") + 1]
            check(artifacts.get(compiler) == identities[0]["compiler_sha256"] and artifacts.get(runtime) == identities[0]["runtime_sha256"], "native build did not use the recorded compiler/runtime artifacts")
    control_passed = False
    if r.get("launcher_control"):
        control_passed = validate_launcher_control(r)
        check(control_passed or not omission, "failed launcher control cannot be an accepted policy omission")
    check(type(r.get("paired")) is bool, "paired must be boolean")
    if r["paired"]: check(bool(eligible) and control_passed, "credited snapshot pair requires an eligible triple and passing launcher control")
    elif eligible: check(isinstance(r.get("credit_reason"), str) and bool(r["credit_reason"]), "uncredited observed intersection needs a credit reason")
    if kind == "script":
        for tier, run in r["runs"].items():
            if run["outcome"] in {"not_run", "omitted"}: continue
            match = re.fullmatch(r"(.+)#L([1-9][0-9]*)", run["evidence"])
            check(bool(match), f"{tier} needs a raw script evidence line")
            path = repository_file(match.group(1)); validate_snapshot_evidence(r, tier, run, path)
            lines = path.read_text().splitlines(); line = int(match.group(2))
            check(line <= len(lines), f"{tier} evidence line missing")
            observed = json.loads(lines[line-1])
            if tier == "stock" and not path.is_relative_to(DATA_ROOT.resolve()):
                check(observed == canonical_original(r, kind), "historical script evidence is not the immutable canonical receipt")
                check(all(observed["runs"]["stock"].get(k) == run.get(k) for k in ("outcome", "version", "exit_code", "stdout_sha256", "stderr_sha256", "effects_sha256")), "historical script stock observations differ")
            else:
                check(observed.get("id") == r["id"] and observed.get("tier") == tier and observed.get("outcome") == run["outcome"], f"{tier} raw script evidence does not establish original ID and outcome")


def validate_snapshot_evidence(r, tier, run, path):
    if tier != "stock":
        require(path.is_relative_to(DATA_ROOT.resolve()), f"{r['id']}: native evidence must belong to this snapshot")
        return
    provenance = run.get("evidence_provenance", {})
    if path.is_relative_to(DATA_ROOT.resolve()):
        require(not provenance or provenance.get("kind") == "fresh", f"{r['id']}: invalid fresh stock evidence label")
        return
    original = canonical_original(r, r["kind"])
    canonical_stock = original.get("runs", {}).get("stock", {})
    matching = canonical_stock.get("outcome") == run["outcome"]
    if r["kind"] == "stdlib": matching = matching and run["evidence"] in {canonical_stock.get("evidence"), canonical_stock.get("raw_evidence")}
    else: matching = matching and path.is_relative_to((HERE / "scripts").resolve())
    require(provenance.get("kind") == "immutable_historical_cpython_3.14.5" and matching, f"{r['id']}: historical stock evidence must be explicitly labeled and match the canonical original")
    stat = path.stat(); digest = provenance.get("sha256")
    require(isinstance(digest, str) and bool(HEX64.fullmatch(digest)) and source_digests(path, stat.st_mtime_ns, stat.st_size)[0] == digest, f"{r['id']}: historical stock evidence digest differs")


def validate_record(r, kind, family_names, frozen_scripts=None):
    label = r.get("id", "<missing id>")
    def check(c, msg): require(c, f"{label}: {msg}")
    check(r.get("schema") == SCHEMA and r.get("kind") == kind, "wrong schema or kind")
    check(isinstance(label, str) and label != "<missing id>" and bool(label), "missing id")
    check(r.get("imre_revision") == COHORT["imre_revision"], "wrong Imre revision")
    check(isinstance(r.get("platform"), dict) and all(r["platform"].get(k) for k in ("os", "arch")), "missing platform")
    check(all(r["platform"].get(k) == v for k, v in COHORT["platform"].items()), "platform differs from pinned cohort")
    source = r.get("source", {})
    check(all(isinstance(source.get(k), str) and source[k] for k in ("repo", "commit", "path", "sha256")), "incomplete source")
    check(bool(HEX40.fullmatch(source["commit"])) and bool(HEX64.fullmatch(source["sha256"])), "invalid source digest")
    if kind == "stdlib":
        check(r.get("family") in family_names, "unknown family")
        check(isinstance(r.get("occurrence"), int) and r["occurrence"] >= 1, "invalid occurrence")
        check(source["repo"] == "https://github.com/python/cpython" and source["commit"] == COHORT["cpython_commit"], "wrong CPython source")
        check(bool(HEX40.fullmatch(source.get("git_blob", ""))), "missing upstream Git blob")
    else:
        invocation = r.get("normalized_invocation", {})
        check(isinstance(invocation.get("argv"), list) and invocation["argv"] and isinstance(invocation.get("cwd"), str) and isinstance(invocation.get("env"), dict), "missing normalized invocation")
    runs = r.get("runs", {})
    check(set(runs) == set(TIERS), "must record stock, O0 and O3")
    for tier in TIERS:
        run = runs[tier]
        check(run.get("outcome") in OUTCOMES, f"invalid {tier} outcome")
        check(isinstance(run.get("argv"), list), f"missing {tier} argv")
        check(bool(run.get("evidence")) if run["outcome"] not in {"not_run", "omitted"} else bool(run.get("reason")), f"missing {tier} evidence/reason")
        if tier == "stock" and run["outcome"] != "not_run":
            check(run.get("version") == COHORT["cpython_version"], "stock must be exact CPython 3.14.5")
        if kind == "script":
            validate_observations(run, lambda c, msg: check(c, f"{tier} {msg}"), run["outcome"] == "pass")
    omission = r.get("omission_reason")
    check("omission_reason" in r and (omission is None or isinstance(omission, str) and bool(omission)), "invalid omission reason")
    check(not any(x["outcome"] == "omitted" for x in runs.values()) or bool(omission), "omission requires reason")
    paired = not omission and all(runs[t]["outcome"] == "pass" for t in TIERS)
    if kind == "script":
        provenance = r.get("fixture_provenance", {})
        check(isinstance(provenance, dict) and provenance.get("status") in {"exact_frozen", "reconstructed", "unknown"}, "missing fixture provenance status")
        observed = provenance.get("preimages", {})
        check(isinstance(observed, dict) and set(observed) <= set(TIERS), "invalid observed preimage tiers")
        for preimages in observed.values(): valid_preimages(preimages, check)
        established = False
        if provenance["status"] == "exact_frozen":
            anchor = (frozen_scripts or {}).get(label)
            check(anchor is not None and anchor.get("source") == source and anchor.get("normalized_invocation") == invocation, "exact frozen fixture requires an independent matching anchor")
            frozen = validate_frozen_anchor(anchor)
            check(set(observed) == set(TIERS), "exact frozen fixture needs preimages for all tiers")
            established = all(observed[t] == frozen["preimages"] for t in TIERS)
            reference = frozen["reference"]
            established = established and all(runs["stock"].get(field) == reference.get(field) for field in ("exit_code", "stdout_sha256", "stderr_sha256", "effects_sha256"))
        paired = paired and established
        comparisons = r.get("comparisons", {})
        check(set(comparisons) == {"O0", "O3"}, "missing comparisons")
        for tier in ("O0", "O3"):
            comparison = comparisons[tier]
            check(set(comparison) == set(DIMENSIONS) and all(v in {"exact", "different", "not_checked"} for v in comparison.values()), f"invalid {tier} comparison")
            for dim in DIMENSIONS:
                status = comparison[dim]
                field = dim if dim == "exit_code" else f"{dim}_sha256"
                a, b = runs["stock"].get(field), runs[tier].get(field)
                if status != "not_checked":
                    check(a is not None and b is not None and ((a == b) == (status == "exact")), f"{tier} {dim} comparison contradicts observations")
            paired = paired and all(v == "exact" for v in comparison.values())
    if is_snapshot():
        validate_snapshot_record(r, kind, paired, check)
    else:
        check(isinstance(r.get("paired"), bool) and r["paired"] == bool(paired), "paired does not match stock/O0/O3 intersection")
    return r


def parse_result(line, module):
    match = RESULT.fullmatch(line)
    if not match: return None
    test_id, status = match.group(2), match.group(3)
    if test_id.startswith("__main__."):
        test_id = module + test_id[len("__main__"):]
    outcome = "pass" if status == "ok" else "skip" if status.startswith("skipped ") else "fail" if status in {"FAIL", "unexpected success", "expected failure"} else "error"
    return test_id, outcome


def source_module(source_path):
    path = Path(source_path)
    require(path.parts[:1] == ("Lib",) and path.suffix == ".py", f"invalid CPython test source path: {source_path}")
    parts = list(path.with_suffix("").parts[1:])
    if parts[-1] == "__init__": parts.pop()
    return ".".join(parts)


@lru_cache(maxsize=None)
def evidence_index(path, module, mtime_ns, size):
    occurrences = Counter(); results = {}
    for line, text in enumerate(path.read_text().splitlines(), 1):
        result = parse_result(text, module)
        if result:
            test_id, outcome = result
            occurrences[test_id] += 1
            results[line] = (test_id, occurrences[test_id], outcome)
    return results



@lru_cache(maxsize=None)
def raw_evidence_index(path, module, mtime_ns, size):
    """Index raw verbose output, including unittest's two-line descriptions."""
    lines = path.read_text().splitlines(); occurrences = Counter(); results = {}
    for index, text in enumerate(lines):
        parsed = parse_result(text, module)
        if parsed is None and index and re.fullmatch(r"[^\s]+ \([^)]+\)", lines[index-1]):
            suffix = re.search(r" \.\.\. (ok|FAIL|ERROR|skipped .+|expected failure|unexpected success)$", text)
            if suffix: parsed = parse_result(lines[index-1] + suffix.group(0), module)
        if parsed:
            test_id, outcome = parsed; occurrences[test_id] += 1
            results[index+1] = (test_id, occurrences[test_id], outcome)
    return results


@lru_cache(maxsize=None)
def source_digests(path, mtime_ns, size):
    data = path.read_bytes()
    return hashlib.sha256(data).hexdigest(), hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()


def verify_stdlib_evidence(r):
    source = r["source"]
    vendored = ROOT / "benchmarks/cpython_stdlib/full" / r["family"] / "upstream" / source["path"]
    if vendored.is_file():
        stat = vendored.stat()
        require(source_digests(vendored, stat.st_mtime_ns, stat.st_size) == (source["sha256"], source["git_blob"]), f"{r['id']}: source hash differs from vendored upstream")
    module = source_module(source["path"])
    for tier, run in r["runs"].items():
        if run["outcome"] not in {"pass", "fail", "error", "skip"} and not (is_snapshot() and run["outcome"] == "timeout"): continue
        evidence = run["evidence"]
        match = re.fullmatch(r"(.+)#L([1-9][0-9]*)", evidence)
        require(bool(match), f"{r['id']}: {tier} needs a raw evidence line")
        path = repository_file(match.group(1))
        if is_snapshot(): validate_snapshot_evidence(r, tier, run, path)
        stat = path.stat(); line = int(match.group(2))
        if run["outcome"] == "timeout":
            lines = path.read_text().splitlines(); observed = json.loads(lines[line-1]) if line <= len(lines) else {}
            result = (observed.get("id"), observed.get("occurrence"), observed.get("outcome")) if observed.get("tier") == tier else None
        else: result = evidence_index(path, module, stat.st_mtime_ns, stat.st_size).get(line)
        require(result == (r["id"], r["occurrence"], run["outcome"]), f"{r['id']}: {tier} evidence does not establish exact ID, occurrence and outcome")
        if is_snapshot() and run["outcome"] != "timeout":
            raw = run.get("raw_evidence", evidence)
            match = re.fullmatch(r"(.+)#L([1-9][0-9]*)", raw)
            require(bool(match), f"{r['id']}: {tier} needs a raw evidence line")
            raw_path = repository_file(match.group(1)); validate_snapshot_evidence(r, tier, {**run, "evidence": raw}, raw_path)
            stat = raw_path.stat(); observed = raw_evidence_index(raw_path, module, stat.st_mtime_ns, stat.st_size).get(int(match.group(2)))
            require(observed == (r["id"], r["occurrence"], run["outcome"]), f"{r['id']}: {tier} raw evidence does not establish exact ID, occurrence and outcome")
            require(raw_path.suffix not in {".results"} or run.get("raw_evidence"), f"{r['id']}: derived results require a raw evidence reference")


def load_records(kind, names, frozen_scripts=None):
    records = []
    seen = set()
    commands = set()
    for path in sorted((DATA_ROOT / ("stdlib" if kind == "stdlib" else "scripts")).glob("*.jsonl")):
        if is_snapshot(): require(path.resolve().is_relative_to(DATA_ROOT.resolve()), f"receipt file escapes snapshot: {path}")
        if kind == "script": require(path.stem in COHORT["scripts"]["shards"], f"unrecognized script shard: {path}")
        for line_num, line in enumerate(path.read_text().splitlines(), 1):
            try: r = validate_record(json.loads(line), kind, names, frozen_scripts)
            except (ValueError, TypeError, KeyError) as e: raise ValueError(f"{path.relative_to(ROOT)}:{line_num}: {e}") from e
            key = (r.get("family"), r["id"], r.get("occurrence")) if kind == "stdlib" else r["id"]
            require(key not in seen, f"duplicate {kind} receipt: {key}")
            seen.add(key)
            if kind == "script":
                command = (r["source"]["repo"], r["source"]["commit"], r["source"]["path"], json.dumps(r["normalized_invocation"], sort_keys=True))
                require(command not in commands, f"duplicate original command: {r['id']}")
                commands.add(command)
            else:
                verify_stdlib_evidence(r)
            records.append(r)
    return records


def summarize_family(row, records):
    require(len(records) <= row["originals"], "receipts exceed raw denominator")
    row["receipts"] = len(records)
    row["observed"] = sum(all(r["runs"][t]["outcome"] == "pass" for t in TIERS) for r in records)
    row["paired"] = sum(r["paired"] for r in records)
    row["runs"] = {t: Counter(r["runs"][t]["outcome"] for r in records) for t in TIERS}
    for t in TIERS: row["runs"][t]["not_run"] += row["originals"] - len(records)


def summary():
    fs = families()
    stdlib = load_records("stdlib", fs)
    scripts = load_records("script", fs, load_frozen_scripts())
    require(len(scripts) <= COHORT["scripts"]["expected_commands"], "more script receipts than original commands")
    grouped = {name: [] for name in fs}
    for r in stdlib: grouped[r["family"]].append(r)
    for name, row in fs.items(): summarize_family(row, grouped[name])
    return fs, scripts


def outcome_text(counts, n):
    if counts["not_run"] == n: return "not run"
    labels = {"pass":"pass", "fail":"fail", "error":"error", "skip":"skip", "timeout":"timeout", "not_run":"not run", "omitted":"omitted"}
    return ", ".join(f"{counts[k]:,} {labels[k]}" for k in labels if counts[k])


def render(view):
    fs, scripts = summary()
    frozen = {n: r for n, r in fs.items() if r["cohort"] == "frozen"}
    supplemental = {n: r for n, r in fs.items() if r["cohort"] == "supplemental"}
    def totals(rows):
        return tuple(sum(r[k] for r in rows.values()) for k in ("originals", "receipts", "observed", "paired"))
    raw, received, observed, paired = totals(frozen)
    sup_raw, sup_received, sup_observed, sup_paired = totals(supplemental)
    expected = COHORT["scripts"]["expected_commands"]
    lines = [START, "### Pinned compatibility scorecard", "", f"Revision `{COHORT['imre_revision']}` · CPython **{COHORT['cpython_version']}** · Linux x86-64. These are original-test receipts at this revision, not cumulative credits.", "", f"Frozen {len(frozen)}-family cohort: **{received:,} pinned receipts** of **{raw:,} raw upstream originals**, **{observed:,} observed stock/O0/O3 pass intersections**, **{paired:,} verified credited pairs**; **{raw-received:,} originals have no receipt**.", "", f"Supplemental pilots, outside the frozen cohort: **{sup_received:,} receipts** of **{sup_raw:,} originals**, **{sup_observed:,} observed intersections**, **{sup_paired:,} verified credited pairs**.", "", f"Scripts: **{len(scripts)} of {expected} original commands have receipts**, **{sum(r['paired'] for r in scripts)} verified paired passes**; **{expected-len(scripts)} not run**."]
    if is_snapshot():
        script_observed = sum(all(r["runs"][t]["outcome"] == "pass" for t in TIERS) for r in scripts)
        lines[-1] = f"Scripts: **{len(scripts)} of {expected} original commands have receipts**, **{script_observed} observed intersections**, **{sum(r['paired'] for r in scripts)} verified credited pairs**; **{expected-len(scripts)} not run**."
    def table(title, rows):
        lines.extend(["", f"#### {title}", "", "| Family | Raw originals | Stock | O0 | O3 | Observed stock/O0/O3 passes | Verified credited pairs | Historical mixed-revision credits |", "| --- | ---: | --- | --- | --- | ---: | --- | ---: |"])
        for name, r in sorted(rows.items()):
            n = r["originals"]
            pair = "unknown (not run)" if not r["receipts"] else f"{r['paired']:,} verified" + (f"; {n-r['receipts']:,} unknown" if r["receipts"] < n else "")
            observed_cell = "unknown (not run)" if not r["receipts"] else f"{r['observed']:,}"
            hist = "—" if r["historical"] is None else f"{r['historical']:,}"
            lines.append(f"| `{name}` | {n:,} | " + " | ".join(outcome_text(r["runs"][t], n) for t in TIERS) + f" | {observed_cell} | {pair} | {hist} |")
    table("Frozen cohort", frozen if view == "stdlib" else {n: r for n, r in frozen.items() if r["receipts"] or n == "argparse"})
    if supplemental: table("Supplemental pilots", supplemental)
    if view == "readme": lines += ["", "The [stdlib report](docs/stdlib.md) lists every frozen family, including those not run."]
    prefix = "../" if view == "stdlib" else ""
    if is_snapshot(): prefix = os.path.relpath(ROOT, DATA_ROOT / "docs" if view == "stdlib" else DATA_ROOT).replace(os.sep, "/") + "/"
    lines += ["", f"An observed intersection means the same original passed stock, O0 and O3; frozen credited pairs also require the launcher to pass its imported bad-type control and any required source-withdrawal check. Script passes also require independently anchored frozen preimages and reference observations, plus exact exit, stdout, stderr and effect comparisons. Skips, omissions, unsupported diagnostics, exclusions, timeouts and non-stock successes never earn credited pairs. Historical credits come from the [frozen mixed-revision ledger]({prefix}{COHORT['historical_families']}) and are separate from the pinned result. [Receipts and replay instructions]({prefix}benchmarks/compatibility/README.md).", END]
    return "\n".join(lines)


def replace_block(text, block):
    require(text.count(START) == text.count(END) == 1, "expected exactly one scorecard block")
    before, rest = text.split(START); _, after = rest.split(END)
    return before + block + after


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--check", action="store_true"); p.add_argument("--validate", action="store_true")
    p.add_argument("--snapshot", metavar="REVISION", help="select revisions/<40-hex-sha> receipt data")
    args = p.parse_args()
    if args.snapshot: select_snapshot(args.snapshot)
    fs, scripts = summary()
    from check_omissions import validate as validate_omissions
    validate_omissions()  # Always validate the canonical omission policy and canonical receipts.
    if args.validate: return
    if is_snapshot():
        require(any(row["receipts"] for row in fs.values()) or scripts, "cannot render a snapshot without genuine receipts")
        outputs = ((DATA_ROOT / "README.md", "readme"), (DATA_ROOT / "docs/stdlib.md", "stdlib"))
        for path, view in outputs:
            new = render(view) + "\n"
            if args.check: require(path.is_file() and path.read_text() == new, f"stale snapshot scorecard: {path.relative_to(ROOT)}")
            else: path.parent.mkdir(parents=True, exist_ok=True); path.write_text(new)
        return
    for path, view in ((ROOT / "README.md", "readme"), (ROOT / "docs/stdlib.md", "stdlib")):
        old = path.read_text(); new = replace_block(old, render(view))
        if args.check: require(old == new, f"stale scorecard: {path.relative_to(ROOT)}")
        else: path.write_text(new)


if __name__ == "__main__": main()
