"""Validate compatibility receipts and render both public scorecard views."""
from __future__ import annotations

import argparse
import ast
import base64
import binascii
import csv
import hashlib
import json
import os
import posixpath
import re
import sys
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
ROOT_SUMMARY_START = "<!-- current-compatibility-snapshot:start -->"
ROOT_SUMMARY_END = "<!-- current-compatibility-snapshot:end -->"


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


def snapshot_activation():
    """Return only explicitly activated, digest-bound snapshot shards."""
    if not is_snapshot(): return {"families": {}, "script_shards": {}}
    path = DATA_ROOT / "activation.json"
    if not path.is_file(): return {"families": {}, "script_shards": {}}
    obj = json.loads(path.read_text())
    require(obj.get("schema") == "imre.compatibility.snapshot-activation.v1" and obj.get("imre_revision") == COHORT["imre_revision"], "invalid snapshot activation revision or schema")
    require(isinstance(obj.get("families"), dict) and isinstance(obj.get("script_shards", {}), dict), "invalid activated shards")
    return obj


def activation_entry(name, kind="stdlib"):
    return snapshot_activation().get("families" if kind == "stdlib" else "script_shards", {}).get(name)


def binding_digest(entry, locator):
    files = entry.get("files", {})
    require(isinstance(files, dict) and isinstance(files.get(locator), str) and bool(HEX64.fullmatch(files[locator])), f"snapshot file is not digest-bound: {locator}")
    return files[locator]


def bound_source(family, role):
    entry = activation_entry(family)
    require(isinstance(entry, dict) and isinstance(entry.get("sources"), dict), f"{family}: missing activation sources")
    locator = entry["sources"].get(role)
    require(isinstance(locator, str), f"{family}: missing bound {role}")
    return checked_file(repository_file(locator, DATA_ROOT), binding_digest(entry, locator))


def validate_receipt_binding(path, entry):
    require(isinstance(entry, dict) and entry.get("receipt") == path.relative_to(ROOT).as_posix(), f"receipt is not explicitly activated: {path}")
    digest = binding_digest(entry, entry["receipt"]); stat = path.stat()
    require(source_digests(path, stat.st_mtime_ns, stat.st_size)[0] == digest, f"activated receipt digest differs: {path}")


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


@lru_cache(maxsize=1)
def canonical_script_shards():
    result = {}
    for shard in CANONICAL_COHORT['scripts']['shards']:
        for line in (HERE/'scripts'/(shard+'.jsonl')).read_text().splitlines():
            id = json.loads(line)['id']
            require(id not in result, 'duplicate canonical script ID')
            result[id] = shard
    require(len(result) == CANONICAL_COHORT['scripts']['expected_commands'], 'canonical script inventory differs')
    return result


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
    relative = path.relative_to(DATA_ROOT)
    if relative.parts[:1] == ("evidence",) and len(relative.parts) >= 3:
        entry = activation_entry(relative.parts[1])
        if entry is not None:
            bound = binding_digest(entry, path.relative_to(ROOT).as_posix())
            require(digest is None or digest == bound, f"manifest and artifact digest differ: {path}")
            digest = bound
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
    build_path, run_path = bound_source(family, "toolchain"), bound_source(family, "run")
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
    control_path = bound_source(family, "gate")
    if run.get("architecture_control") is not None: require(run["architecture_control"] == control_path.relative_to(ROOT).as_posix(), "run references another launcher control")
    control = artifact_json(control_path)
    copied = "graph_build" in control
    graph = control.get("graph_build", control); argv = graph.get("argv"); runtime = runtime_argument(argv)
    require(argv[0] == identity[0] and runtime == identity[2] and option(argv, "--runtime") == identity[2] and option(argv, "--opt-level") == "0" and "--no-cache" in argv and control.get("imre_revision", revision) == revision, "launcher control did not use O0 and the exact pinned compiler/runtime")
    require(comparable_build(argv, copied) == comparable_build(tiers["O0"]["build_argv"], copied), "poisoned build flags differ from the positive O0 launcher")
    poison = bound_source(family, "poison_source")
    checked_file(poison, control.get("control_module_sha256", control.get("poison_sha256")))
    require(poison.read_bytes() == b"status: int = True\n", "control source is not the imported bad-type assignment")
    if copied:
        require(control.get("positive_launcher_sha256") == source_sha and control.get("saved_images_copied_before_runner_cleanup") is True, "control does not bind the original positive launcher")
        original = ROOT / "benchmarks/cpython_stdlib/full" / family / "upstream" / source_path
        require(original.is_file() and hashlib.sha256(original.read_bytes()).hexdigest() == source_sha, "positive FILE source is unavailable or altered")
        poisoned = bound_source(family, "poisoned_launcher"); checked_file(poisoned, control.get("poisoned_launcher_sha256"))
        import_line = f"import {poison.stem}\n".encode(); lines = original.read_bytes().splitlines(keepends=True)
        require(lines and poisoned.read_bytes() == lines[0] + import_line + b"".join(lines[1:]) and control.get("launcher_change") == f"insert import {poison.stem} after the module docstring; otherwise unchanged original launcher", "poisoned FILE copy or explicit import injection differs")
        positive_root, poisoned_root = option(tiers["O0"]["build_argv"], "--module-root"), option(argv, "--module-root")
        require(Path(launcher).is_relative_to(positive_root) and Path(argv[2]).is_relative_to(poisoned_root) and Path(launcher).relative_to(positive_root) == Path(argv[2]).relative_to(poisoned_root), "poisoned FILE copy has a different module entry")
        stderr = checked_file(bound_source(family, "gate_stderr"), graph.get("stderr_sha256")).read_text().lower(); image_created = graph.get("executable_exists")
    else:
        positive = bound_source(family, "positive_launcher"); checked_file(positive, control.get("positive_launcher_sha256"))
        injected = f"import {poison.stem}\n".encode() + positive.read_bytes()
        require(control.get("positive_launcher") == launcher == argv[2] and control.get("launcher_restored_by_hash") is True and control.get("injection") == f"import {poison.stem} prepended to the actual positive launcher, then restored by hash" and hashlib.sha256(injected).hexdigest() == control.get("control_launcher_sha256"), "control does not bind the positive launcher and import injection")
        stderr = bound_source(family, "gate_stderr").read_text().lower(); image_created = control.get("image_existed")
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
        require(bound_source(family, "withdrawal") == control_path, "embedded withdrawal binding differs")
        require(isinstance(withdrawal, dict) and withdrawal.get("source_absent_during_runs") is True and withdrawal.get("matching_bytecode_absent") is True and withdrawal.get("source_sha256_before") == source_sha == withdrawal.get("source_sha256_restored"), "source withdrawal did not establish the original source")
        for tier in ("O0", "O3"):
            witness = withdrawal.get("runs", {}).get(tier, {}); meta = tiers[tier]; saved = meta.get("saved_image")
            require(witness.get("exit_code") == 0 and witness.get("image_sha256") == meta.get("executable_sha256") and witness.get("image_matches_saved_baseline_hash") is True and saved in witness.get("argv", []) and witness.get("exact_id_occurrence_outcome_sequence_matches_baseline") is True and witness.get("additional_exec_calls") == [] and witness.get("linked_libpython") is False and witness.get("ldd_exit_code") == 0, f"{tier} saved-image check failed")
            trace_path = checked_file(bound_source(family, f"withdrawal_{tier}_trace"), witness.get("trace_sha256")); trace = trace_path.read_text()
            calls = [line for line in trace.splitlines() if re.search(r"execve(?:at)?\(", line)]
            require(len(calls) == 1 and re.search(r"= 0(?:\s|$)", calls[0]) and calls == witness.get("exec_calls") and f'"{saved}"' in calls[0], f"{tier} child exec observed or saved image differs")
            require("libpython" not in bound_source(family, f"withdrawal_{tier}_link").read_text().lower(), f"{tier} linked libpython")
            outcomes = checked_file(bound_source(family, f"withdrawal_{tier}_outcomes"), witness.get("stderr_sha256")); baseline = directory / f"{tier}.stderr"
            sequence = raw_sequence(outcomes, module)
            require(sequence and sequence == raw_sequence(baseline, module) and len(sequence) == witness.get("completed_originals"), f"{tier} source-withdrawn outcomes differ")
            references["saved_checks"][tier] = {"binary_sha256": witness["image_sha256"], "outcomes": file_ref(outcomes), "trace": file_ref(trace_path)}
    else:
        withdrawal_path = bound_source(family, "withdrawal"); withdrawal = artifact_json(withdrawal_path)
        require(withdrawal.get("performed") is True and withdrawal.get("source_absent") is True and withdrawal.get("bytecode") == [] and withdrawal.get("restored_by_hash") is True and withdrawal.get("source_sha256_before") == source_sha == withdrawal.get("source_sha256_after") and set(withdrawal.get("runs", [])) == {"O0", "O3"}, "source withdrawal did not establish the original source")
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


def gate_diagnostic(path, digest, exit_code, timed_out, image_created):
    stderr = checked_file(path, digest).read_text().lower()
    require(type(exit_code) is int and exit_code != 0 and timed_out is False and image_created is False and bool(re.search(r"expected\s+.*?int.*?(?:got|found)\s+.*?bool", stderr, re.S)), "imported bad-store gate did not reject without an image")


def matched_build(argv, tier, compiler, runtime):
    require(runtime_argument(argv) == runtime and argv[0] == compiler and option(argv, "--runtime") == runtime and option(argv, "--opt-level") == tier[1:] and "--no-cache" in argv, f"{tier} compiler/runtime or build flags differ")


def successful_open_paths(text, cwd, *, preserve_traversal=False, kernel_events=False):
    """Resolve strace opens; optionally emit strict -y lexical/kernel path events."""
    descriptors = {}; previous = {}; quoted = r'"(?:\\.|[^"\\])*"'
    def decode(value, annotation=False):
        literal = '"' + value + '"' if annotation else value
        try:
            return os.fsdecode(ast.literal_eval('b'+literal)) if kernel_events else ast.literal_eval(literal)
        except (ValueError, SyntaxError, UnicodeError):
            require(False, "invalid open path in saved-image trace")
    def path_for(dirfd, literal, pid, operation):
        name = decode(literal); base = cwd
        if operation not in {"open","creat"}:
            raw = dirfd.strip(); annotated = re.fullmatch(r'([^<>]+)<([^<>]+)>',raw)
            if kernel_events:
                require(annotated is not None and (annotated[1] == "AT_FDCWD" or annotated[1].isdigit()), "unknown kernel dirfd")
                base = decode(annotated[2],True)
            else:
                base = cwd if raw == "AT_FDCWD" else descriptors.get((pid,raw.split("<",1)[0]))
                if base is None and annotated: base = annotated[2]
        if kernel_events and operation in {"open","creat"}: require(name.startswith("/"), "unresolved relative open without kernel cwd")
        require(isinstance(name,str) and (name.startswith("/") or isinstance(base,str) and base.startswith("/")), "unresolved successful relative open in saved-image trace")
        return name if name.startswith("/") else posixpath.join(base,name)
    for number,line in enumerate(text.splitlines(),1):
        pm = re.match(r"\s*(?:\[pid\s+(\d+)\]|(\d+))\s+",line)
        pid = (pm.group(1) or pm.group(2)) if pm else "main"; prior=previous.get(pid);previous[pid]=line
        call = re.search(r"\b(open|openat|openat2|creat)\((.*)\)\s+=\s+(-?\d+)(?:<([^<>]*)>)?(?:\s|$)",line)
        if kernel_events and not call and re.search(r"\b(?:open|openat|openat2|creat)\(|<\.\.\. (?:open|openat|openat2|creat) resumed>",line):
            require(bool(re.search(r"\)\s+=\s+-1\b",line)), "unresolved native open event")
        if not call or int(call[3]) < 0 or (call[1]=="creat" and not kernel_events): continue
        op,args,fd=call[1],call[2],int(call[3]); dirfd=None
        if op in {"open","creat"}: match=re.match(rf"({quoted})(?:,|$)(.*)",args)
        else: match=re.match(rf"([^,]+),\s*({quoted})(?:,|$)(.*)",args);dirfd=match[1].strip() if match else None
        require(match is not None,"cannot decode successful open in saved-image trace")
        literal=match[1] if op in {"open","creat"} else match[2]
        joined=path_for(dirfd,literal,pid,op);normalized=posixpath.normpath(joined)
        descriptors[(pid,str(fd))]=joined if preserve_traversal else normalized
        if not kernel_events:
            yield Path(joined if preserve_traversal else normalized);continue
        require(call[4] is not None,"successful open has no kernel fd path")
        returned=decode(call[4],True);require(returned.startswith("/") and not returned.endswith(" (deleted)"),"invalid kernel fd path")
        rest=(match[2] if op in {"open","creat"} else match[3]).strip()
        if op=="openat2":
            fm=re.search(r'\bflags=([^,}]+)',rest);require(fm is not None,"openat2 flags missing");flags=fm[1];mode=None
        elif op=="creat": flags="CREAT";mode=rest
        else:
            parts=rest.split(",",1);flags=parts[0].strip();mode=parts[1].strip() if len(parts)>1 else None
        witness=None
        if prior and "S_IFDIR" in prior and re.search(r'\) = 0$',prior):
            sm=re.search(rf'\bnewfstatat\(([^,]+),\s*({quoted}),',prior)
            if sm: witness=posixpath.normpath(path_for(sm[1],sm[2],pid,"openat"))
        yield {"line":number,"pid":pid,"operation":op,"lexical":joined,"path":Path(normalized),
               "returned":Path(posixpath.normpath(returned)),"flags":flags,"mode":mode,"directory_witness":witness}


def validate_trace(path, digest, image, source_leaf, source_path, cwd, forbidden_trees=()):
    text = checked_file(path, digest).read_text()
    calls = [line for line in text.splitlines() if re.search(r"execve(?:at)?\(", line)]
    require(len(calls) == 1 and f'"{image}"' in calls[0] and re.search(r"= 0(?:\s|$)", calls[0]) and "libpython" not in text.lower(), "saved image spawned a child or accessed libpython")
    leaf = Path(posixpath.normpath(source_leaf)); cache = leaf.parent / "__pycache__"
    bytecode = {leaf.with_suffix(".pyc"), leaf.with_suffix(".pyo")}
    trees = tuple(Path(posixpath.normpath(str(tree))) for tree in forbidden_trees)
    canonical = Path(source_path); suffixes = (canonical.parts, canonical.with_suffix(".pyc").parts, canonical.with_suffix(".pyo").parts); cache_parts = (*canonical.parent.parts, "__pycache__")
    for opened in successful_open_paths(text, cwd):
        parts = opened.parts; same_leaf = opened.name in {leaf.name, leaf.with_suffix(".pyc").name, leaf.with_suffix(".pyo").name} or any(parts[-len(suffix):] == suffix for suffix in suffixes)
        matching_cached_bytecode = opened.parent.name == "__pycache__" and opened.name.startswith(leaf.stem + ".") and opened.suffix in {".pyc", ".pyo"}
        same_cache = any(parts[i:i+len(cache_parts)] == cache_parts for i in range(len(parts)-len(cache_parts)+1))
        require(opened != leaf and opened not in bytecode and not opened.is_relative_to(cache) and not same_leaf and not same_cache and not matching_cached_bytecode and not any(opened.is_relative_to(tree) for tree in trees), f"saved image successfully opened test source, bytecode or withdrawn test tree: {opened}")


@lru_cache(maxsize=None)
def adapt_pathlib(root, revision, source_path, source_sha, source_blob):
    family = "pathlib"; directory = root / "evidence" / family
    gate_path, run_path = bound_source(family, "gate"), bound_source(family, "run")
    require(bound_source(family, "toolchain") == gate_path and bound_source(family, "withdrawal") == run_path, "pathlib toolchain/withdrawal bindings differ")
    gate, run = artifact_json(gate_path), artifact_json(run_path); control = gate.get("control", {})
    require(gate.get("imre_revision") == revision == gate.get("build_worktree_head") and gate.get("build_worktree_clean") is True and gate.get("cpython_commit") == COHORT["cpython_commit"], "pathlib toolchain source identity differs")
    require(all(isinstance(gate.get(k), str) and HEX64.fullmatch(gate[k]) for k in ("imre_sha256", "runtime_sha256")), "pathlib compiler/runtime hashes missing")
    require(run.get("gate") == gate_path.relative_to(ROOT).as_posix() and run.get("family") == family, "pathlib gate reference differs")
    argv = control.get("argv"); compiler, runtime = argv[0], runtime_argument(argv); matched_build(argv, "O0", compiler, runtime)
    poison = bound_source(family, "poison_source"); checked_file(poison, control.get("control_module_sha256"))
    module = Path(control.get("control_module", "")).stem
    positive = bound_source(family, "positive_launcher"); stat = positive.stat()
    require(source_digests(positive, stat.st_mtime_ns, stat.st_size) == (source_sha, source_blob), "pathlib positive launcher SHA256 or Git blob differs from canonical CPython source")
    negative_sha = hashlib.sha256(f"import {module}\n".encode() + positive.read_bytes()).hexdigest()
    require(poison.read_bytes() == b"status: int = True\n" and module and control.get("transformation") == f"prepend import {module} followed by newline to the exact positive launcher at the same path" and control.get("positive_launcher_sha256") == source_sha == control.get("restored_launcher_sha256") and control.get("launcher_negative_sha256") == negative_sha and control.get("launcher") == argv[2], "pathlib poisoned launcher or import binding differs")
    gate_diagnostic(directory / "gate.stderr", control.get("stderr_sha256"), control.get("exit_code"), control.get("timed_out"), control.get("image_exists"))
    w = run.get("withdrawal", {})
    require(w.get("test_root_absent") is True and w.get("restored_by_hash") is True and w.get("leaf_sha256_before") == source_sha == w.get("leaf_sha256_after") and w.get("sources_before") == w.get("sources_after") and w.get("sources_before", {}).get("files", 0) > 0, "pathlib source/bytecode tree withdrawal missing")
    module_root = Path(option(argv, "--module-root")); leaf = Path(argv[2]); source_parts = Path(source_path).parts
    require(source_parts[:2] == ("Lib", "test") and leaf == module_root.joinpath(*source_parts[1:]) and module_root.is_absolute() and Path(w.get("withdrawn_root", "")).is_absolute(), "pathlib source or withdrawn tree path differs")
    test_root = module_root / "test"
    mod = source_module(source_path); stock = raw_sequence(directory / "stock.stderr", mod)
    checked_file(directory / "stock.stderr", run.get("stock", {}).get("stderr_sha256"))
    for tier in ("O0", "O3"):
        meta = run.get("tiers", {}).get(tier, {}); build = meta.get("build", {}); pos = build.get("argv"); matched_build(pos, tier, compiler, runtime)
        require(build.get("exit_code") == 0 and pos[2] == argv[2] and (tier != "O0" or comparable_build(pos) == comparable_build(argv)) and meta.get("image_exists") is True and bool(HEX64.fullmatch(meta.get("image_sha256", ""))), f"{tier} pathlib positive build differs")
        image = option(pos, "-o"); baseline, withdrawn = meta.get("baseline", {}), meta.get("withdrawn", {})
        for label, observation in (("baseline", baseline), ("withdrawn", withdrawn)):
            require(observation.get("exit_code") == 0 and observation.get("timed_out") is False and image in observation.get("argv", []) and observation.get("initial_image_exec_only") is True and observation.get("libpython_access") is False and observation.get("test_source_access") is False, f"{tier} pathlib {label} run differs")
            trace = repository_file(observation.get("trace"), root)
            trees = (test_root, w["withdrawn_root"]) if label == "withdrawn" else ()
            validate_trace(trace, observation.get("trace_sha256"), image, str(leaf), source_path, observation.get("cwd"), trees)
            raw = checked_file(directory / f"{tier}.{label}.stderr", observation.get("stderr_sha256"))
            require(raw_sequence(raw, mod) == stock and stock, f"{tier} pathlib {label} exact outcomes differ")
        readelf = checked_file(directory / f"{tier}.readelf.stdout", meta.get("readelf", {}).get("stdout_sha256"))
        require(meta.get("readelf", {}).get("exit_code") == 0 and meta.get("libpython_needed") is False and "libpython" not in readelf.read_text().lower(), f"{tier} pathlib links libpython")
    return run


@lru_cache(maxsize=None)
def adapt_urllib(root, revision, source_path, source_sha, source_blob):
    family = "urllib"; directory = root / "evidence" / family
    build, gate = artifact_json(bound_source(family, "toolchain")), artifact_json(bound_source(family, "gate"))
    withdrawal, controls = artifact_json(bound_source(family, "withdrawal")), artifact_json(bound_source(family, "controls"))
    require(build.get("head") == revision and build.get("clean") is True and build.get("exit_code") == 0 and gate.get("imre_revision") == revision and controls.get("cpython_source_commit") == COHORT["cpython_commit"], "urllib toolchain or gate revision differs")
    argv = gate.get("argv"); compiler, runtime = argv[0], runtime_argument(argv); matched_build(argv, "O0", compiler, runtime)
    artifacts = build.get("artifacts", {}); require(all(isinstance(artifacts.get(k), str) and HEX64.fullmatch(artifacts[k]) for k in (compiler, runtime)), "urllib compiler/runtime identities missing")
    positive, poisoned, poison = (bound_source(family, role) for role in ("positive_launcher", "poisoned_launcher", "poison_source"))
    checked_file(positive, gate.get("positive_launcher_sha256")); checked_file(poisoned, gate.get("poisoned_launcher_sha256")); checked_file(poison, gate.get("poison_source_sha256"))
    lines = positive.read_bytes().splitlines(keepends=True); injection = f"import {poison.stem}\n".encode()
    require(poison.read_bytes() == b"status: int = True\n" and lines and poisoned.read_bytes() == lines[0] + injection + b"".join(lines[1:]) and f"import {poison.stem}" in gate.get("change", ""), "urllib poisoned launcher or explicit import differs")
    gate_diagnostic(directory / "gate/stderr", gate.get("stderr_sha256"), gate.get("exit_code"), gate.get("timed_out"), gate.get("output_exists_after"))
    name = Path(source_path).name; module_key = "urlparse" if name == "test_urlparse.py" else "urllib" if name == "test_urllib.py" else None
    require(module_key is not None, "unknown urllib source module")
    module_run = artifact_json(bound_source(family, "module_" + module_key)); module_dir = directory / module_key
    require(module_run.get("source") == {"repo":"https://github.com/python/cpython", "commit":COHORT["cpython_commit"], "path":source_path, "sha256":source_sha, "git_blob":source_blob}, "urllib module source differs")
    require(withdrawal.get("before_source_sha256", {}).get(name) == source_sha == withdrawal.get("restored_source_sha256", {}).get(name) and withdrawal.get("restored_by_hash") is True and withdrawal.get("matching_bytecode_before") == [] and withdrawal.get("fixture_files_before", 0) - withdrawal.get("fixture_files_during", 0) == len(withdrawal.get("before_source_sha256", {})), "urllib source or bytecode withdrawal missing")
    child, lib = controls.get("child_exec_probe", {}), controls.get("libpython_probe", {})
    require(child.get("exit_code") == 0 and "EPERM" in child.get("observed", "") and lib.get("exit_code") == 126 and "detected" in lib.get("observed", "") and "denied with EPERM" in checked_file(directory / "controls/probe.denyexec.log").read_text() and "libpython" in checked_file(directory / "controls/probe.auditlibs.log").read_text().lower(), "urllib child-exec/libpython probes missing")
    mod = source_module(source_path); stock_meta = module_run.get("stock", {}); stock_path = checked_file(module_dir / "stock.stderr", stock_meta.get("stderr_sha256")); stock = raw_sequence(stock_path, mod)
    require(stock and stock_meta.get("fresh_run") is True and stock_meta.get("version") == COHORT["cpython_version"], "urllib fresh stock evidence missing")
    for tier in ("O0", "O3"):
        meta = module_run.get("native", {}).get(tier, {}); pos = meta.get("build_argv"); matched_build(pos, tier, compiler, runtime)
        image = withdrawal.get("images", {}).get(tier, {}); witness = withdrawal.get("runs", {}).get(f"{tier}.{module_key}", {})
        require(meta.get("build_exit_code") == 0 and pos[2] == argv[2] and (tier != "O0" or comparable_build(pos) == comparable_build(argv)) and option(pos, "-o") == image.get("path") == meta.get("argv", [None])[0] and image.get("sha256") == meta.get("image_sha256") and bool(HEX64.fullmatch(image.get("sha256", ""))) and witness.get("argv") == meta.get("argv") and witness.get("stderr_sha256") == meta.get("stderr_sha256") and witness.get("source_files_absent") is True and witness.get("matching_bytecode_absent") is True and witness.get("exit_code") == meta.get("exit_code") == stock_meta.get("exit_code") and witness.get("exit_code") in (0, 1) and witness.get("timed_out") is False, f"{tier} urllib saved image or withdrawal differs")
        raw = checked_file(module_dir / f"{tier}.stderr", witness.get("stderr_sha256")); require(raw_sequence(raw, mod) == stock, f"{tier} urllib exact withdrawn outcomes differ")
        denial = checked_file(module_dir / f"{tier}.denyexec.log", witness.get("denyexec_sha256")).read_text(); audit = checked_file(module_dir / f"{tier}.auditlibs.log", witness.get("auditlibs_sha256")).read_text().lower()
        readelf = checked_file(directory / f"{tier}.readelf.stdout")
        require(denial == "execve and execveat denied with EPERM\n" and "<main executable>" in audit and "libpython" not in audit and image.get("readelf_exit_code") == 0 and image.get("libpython_needed") is False and "libpython" not in readelf.read_text().lower(), f"{tier} urllib child exec or libpython evidence differs")
    return module_run


def validate_activated_family_record(r, eligible, check, entry):
    adapter = entry.get("adapter"); source = r["source"]; family = r["family"]; d = DATA_ROOT / "evidence" / family
    if adapter == "raw-control-v1":
        import raw_control
        return raw_control.validate_record(sys.modules[__name__], r, eligible, check, entry)
    if adapter == "shared-family-v1": return validate_family_artifact_record(r, eligible, check)
    if adapter == "pathlib-leaf-v1":
        manifest = adapt_pathlib(DATA_ROOT, COHORT["imre_revision"], source["path"], source["sha256"], source["git_blob"]); stock = manifest["stock"]
        for tier in ("O0", "O3"):
            run = r["runs"][tier]; meta = manifest["tiers"][tier]
            check(run.get("argv") == meta["baseline"].get("argv") and run.get("evidence", "").split("#L")[0] == (d / f"{tier}.baseline.stderr").relative_to(ROOT).as_posix(), f"{tier} pathlib receipt run differs")
    elif adapter == "urllib-modules-v1":
        manifest = adapt_urllib(DATA_ROOT, COHORT["imre_revision"], source["path"], source["sha256"], source["git_blob"]); stock = manifest["stock"]; module_dir = d / ("urlparse" if Path(source["path"]).name == "test_urlparse.py" else "urllib")
        for tier in ("O0", "O3"):
            run = r["runs"][tier]; meta = manifest["native"][tier]
            check(run.get("argv") == meta.get("argv") and run.get("build_argv") == meta.get("build_argv") and run.get("evidence", "").split("#L")[0] == (module_dir / f"{tier}.stderr").relative_to(ROOT).as_posix(), f"{tier} urllib receipt run differs")
    else: check(False, f"unknown snapshot adapter: {adapter}")
    check(r["runs"]["stock"].get("argv") == stock.get("argv"), "stock receipt launcher differs")
    check(type(r.get("paired")) is bool and (not r["paired"] or eligible), "credited pair requires exact passing original outcomes")
    if eligible and not r["paired"]: check(isinstance(r.get("credit_reason"), str) and bool(r["credit_reason"]), "uncredited observed intersection needs a credit reason")


def validate_snapshot_record(r, kind, eligible, check):
    canonical_original(r, kind)
    omission = r.get("omission_reason")
    if omission:
        check(kind == "stdlib" and r.get("policy_omission") == accepted_policy_locator(r) and r.get("policy_omission") is not None, "snapshot omission is not an accepted canonical policy omission")
    entry = activation_entry(r.get("family")) if kind == "stdlib" else activation_entry(canonical_script_shards().get(r['id']), 'script')
    if COHORT['imre_revision'] == '95908fb0acadc2a67304a8080e584deaef395746' and (kind == 'script' or r.get('family') == 'fnmatch'):
        import script_control
        script_control.require_paired_authority(sys.modules[__name__],r,entry)
    if entry and kind == "stdlib":
        import raw_control
        if COHORT["imre_revision"] == raw_control.SOURCE_PIN and r["family"] in raw_control.SOURCE_FAMILIES:
            raw_control.source_stage_authority(sys.modules[__name__], r["family"], raw_control.sha(json.dumps(entry, sort_keys=True).encode()))
    if entry and kind == 'stdlib' and entry.get("adapter") != "row-v1":
        validate_activated_family_record(r, eligible, check, entry)
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
    if entry and kind == 'script' and entry.get('adapter') != 'row-v1':
        import script_control
        check(entry.get('adapter') == 'script-control-v1', 'unknown script snapshot adapter')
        return script_control.validate_record(sys.modules[__name__], r, eligible, check, entry)
    check(type(r.get("paired")) is bool, "paired must be boolean")
    if r["paired"]: check(False, "row-v1 credited pairs are unsupported until raw saved-image proof has a validated schema")
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
    if is_snapshot() and (activation_entry(r["family"]) or {}).get("adapter") == "raw-control-v1":
        return  # validate_snapshot_record already verified raw finite-runner events and locators.
    module = source_module(source["path"])
    for tier, run in r["runs"].items():
        if run["outcome"] not in {"pass", "fail", "error", "skip"} and not (is_snapshot() and run["outcome"] == "timeout"): continue
        evidence = run["evidence"]
        match = re.fullmatch(r"(.+)#L([1-9][0-9]*)", evidence)
        require(bool(match), f"{r['id']}: {tier} needs a raw evidence line")
        path = repository_file(match.group(1))
        if is_snapshot():
            validate_snapshot_evidence(r, tier, run, path)
            if path.is_relative_to(DATA_ROOT.resolve()) and activation_entry(r["family"]): checked_file(path, binding_digest(activation_entry(r["family"]), path.relative_to(ROOT).as_posix()))
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
            if raw_path.is_relative_to(DATA_ROOT.resolve()) and activation_entry(r["family"]): checked_file(raw_path, binding_digest(activation_entry(r["family"]), raw_path.relative_to(ROOT).as_posix()))
            stat = raw_path.stat(); observed = raw_evidence_index(raw_path, module, stat.st_mtime_ns, stat.st_size).get(int(match.group(2)))
            require(observed == (r["id"], r["occurrence"], run["outcome"]), f"{r['id']}: {tier} raw evidence does not establish exact ID, occurrence and outcome")
            require(raw_path.suffix not in {".results"} or run.get("raw_evidence"), f"{r['id']}: derived results require a raw evidence reference")


def activated_subset_path(staged, entry, kind):
    """Select a full shard or an explicitly separated verified subset."""
    if not entry.get('staged_receipt'):
        require(entry.get('receipt') == staged.relative_to(ROOT).as_posix(), f"activated shard path differs: {staged}")
        return staged, None
    locator = staged.relative_to(ROOT).as_posix()
    require(entry.get('staged_receipt') == locator, f"staged shard path differs: {staged}")
    checked_file(staged, binding_digest(entry, locator))
    verified = DATA_ROOT / 'verified' / ('stdlib' if kind == 'stdlib' else 'scripts') / staged.name
    require(entry.get('receipt') == verified.relative_to(ROOT).as_posix() and verified.is_file() and verified.resolve().is_relative_to(DATA_ROOT.resolve()), f"verified subset path differs: {verified}")
    originals = {}
    for line in staged.read_text().splitlines():
        row = json.loads(line); key = (row.get('id'), row.get('occurrence')) if kind == 'stdlib' else row.get('id')
        valid = row.get('family') == staged.stem and isinstance(key[0],str) and type(key[1]) is int if kind == 'stdlib' else isinstance(key,str) and canonical_script_shards().get(key)==staged.stem
        require(valid and key not in originals, f"invalid or duplicate staged original: {staged}")
        canonical_original(row, kind)
        originals[key] = row
    return verified, originals


def activated_stdlib_path(staged, entry):
    return activated_subset_path(staged,entry,'stdlib')


def load_records(kind, names, frozen_scripts=None):
    records = []
    seen = set()
    commands = set()
    for staged in sorted((DATA_ROOT / ("stdlib" if kind == "stdlib" else "scripts")).glob("*.jsonl")):
        path, originals = staged, None
        if is_snapshot():
            require(staged.resolve().is_relative_to(DATA_ROOT.resolve()), f"receipt file escapes snapshot: {staged}")
            entry = activation_entry(staged.stem, kind)
            if entry is None: continue
            path, originals = activated_subset_path(staged, entry, kind)
            validate_receipt_binding(path, entry)
        if kind == "script": require(path.stem in COHORT["scripts"]["shards"], f"unrecognized script shard: {path}")
        for line_num, line in enumerate(path.read_text().splitlines(), 1):
            try: r = validate_record(json.loads(line), kind, names, frozen_scripts)
            except (ValueError, TypeError, KeyError) as e: raise ValueError(f"{path.relative_to(ROOT)}:{line_num}: {e}") from e
            if is_snapshot() and kind == "stdlib": require(r.get("family") == path.stem, f"receipt family differs from activated shard: {path}")
            if originals is not None:
                original = originals.get((r['id'],r['occurrence']) if kind == 'stdlib' else r['id'])
                fields=('source','platform','imre_revision','kind','family','id','occurrence') if kind=='stdlib' else ('source','platform','imre_revision','kind','id','normalized_invocation')
                require(original is not None and all(r.get(k) == original.get(k) for k in fields), f"verified original identity differs from staged shard: {r['id']}")
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
    for name, row in fs.items():
        summarize_family(row, grouped[name])
        if is_snapshot():
            staged = DATA_ROOT / "stdlib" / f"{name}.jsonl"
            entry = activation_entry(name)
            row['pending'] = 0
            if staged.is_file() and entry is None:
                row['pending'] = sum(bool(line.strip()) for line in staged.read_text().splitlines())
            elif entry and entry.get('staged_receipt'):
                _, originals = activated_stdlib_path(staged, entry)
                active = {(r['id'],r['occurrence']) for r in grouped[name]}
                staged_originals = entry.get('staged_originals', row['originals'])
                require(type(staged_originals) is int and 0 < staged_originals <= row['originals']
                        and len(originals) == staged_originals, f'{name}: partial staged shard does not cover frozen denominator or declared staged count')
                if staged_originals < row['originals']:
                    catalog = {key[1:] for key in canonical_originals('stdlib') if key[0] == name}
                    require(len(catalog) == row['originals'] and set(originals) <= catalog,
                            f'{name}: sparse stage differs from complete canonical original catalog')
                require(active <= set(originals), f'{name}: verified subset exceeds staged identities')
                pending = [r for key,r in originals.items() if key not in active]
                row['pending'] = len(pending)
                for t in TIERS: row['runs'][t]['not_run'] -= len(pending)
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
    missing = "originals have no validated receipt" if is_snapshot() else "originals have no receipt"
    lines = [START, "### Pinned compatibility scorecard", "", f"Revision `{COHORT['imre_revision']}` · CPython **{COHORT['cpython_version']}** · Linux x86-64. These are original-test receipts at this revision, not cumulative credits.", "", f"Frozen {len(frozen)}-family cohort: **{received:,} pinned receipts** of **{raw:,} raw upstream originals**, **{observed:,} observed stock/O0/O3 pass intersections**, **{paired:,} verified credited pairs**; **{raw-received:,} {missing}**.", "", f"Supplemental pilots, outside the frozen cohort: **{sup_received:,} receipts** of **{sup_raw:,} originals**, **{sup_observed:,} observed intersections**, **{sup_paired:,} verified credited pairs**.", "", f"Scripts: **{len(scripts)} of {expected} original commands have receipts**, **{sum(r['paired'] for r in scripts)} verified paired passes**; **{expected-len(scripts)} not run**."]
    if is_snapshot():
        script_observed = sum(all(r["runs"][t]["outcome"] == "pass" for t in TIERS) for r in scripts)
        lines[-1] = f"Scripts: **{len(scripts)} of {expected} original commands have receipts**, **{script_observed} observed intersections**, **{sum(r['paired'] for r in scripts)} verified credited pairs**; **{expected-len(scripts)} not run**."
    def table(title, rows):
        lines.extend(["", f"#### {title}", "", "| Family | Raw originals | Stock | O0 | O3 | Observed stock/O0/O3 passes | Verified credited pairs | Historical mixed-revision credits |", "| --- | ---: | --- | --- | --- | ---: | --- | ---: |"])
        for name, r in sorted(rows.items()):
            n = r["originals"]
            if is_snapshot() and r.get('pending'):
                pair = f"{r['paired']:,} verified; {r['pending']:,} staged, unverified" if r['receipts'] else f"pending ({r['pending']:,} staged, unverified)"
            else:
                pair = "unknown (not run)" if not r['receipts'] else f"{r['paired']:,} verified" + (f"; {n-r['receipts']:,} unknown" if r['receipts'] < n else '')
            observed_cell = "unknown (not run)" if not r["receipts"] else f"{r['observed']:,}"
            hist = "—" if r["historical"] is None else f"{r['historical']:,}"
            outcomes = []
            for t in TIERS:
                text = outcome_text(r['runs'][t], n)
                if r.get('pending') and r['receipts']:
                    text = (text + '; ' if text else '') + f"{r['pending']:,} staged (unverified; outcomes not checked)"
                outcomes.append(text)
            lines.append(f"| `{name}` | {n:,} | " + " | ".join(outcomes) + f" | {observed_cell} | {pair} | {hist} |")
    table("Frozen cohort", frozen if view == "stdlib" else {n: r for n, r in frozen.items() if r["receipts"] or r.get("pending") or n == "argparse"})
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


def render_root_summary(fs):
    """Render the explicitly selected snapshot's staged modules for the root README."""
    require(is_snapshot(), "root summary requires a snapshot")
    pin = COHORT["imre_revision"]
    staged = {name: row for name, row in fs.items() if row["receipts"] or row.get("pending")}
    lines = [ROOT_SUMMARY_START, "### Current compatibility snapshot", "", f"Revision `{pin}` · **{sum(r['receipts'] for r in staged.values()):,} validated receipts**, **{sum(r['paired'] for r in staged.values()):,} verified pairs**.", "", "| Module | Raw originals | Verified pairs / status |", "| --- | ---: | --- |"]
    for name, row in sorted(staged.items()):
        status = ((f"{row['paired']:,} verified; " if row['receipts'] else '') + f"pending ({row['pending']:,} staged)") if row.get('pending') else f"{row['paired']:,} verified"
        lines.append(f"| `{name}` | {row['originals']:,} | {status} |")
    lines.extend(["", f"[Full revision table](benchmarks/compatibility/revisions/{pin}/docs/stdlib.md)", ROOT_SUMMARY_END])
    return "\n".join(lines)


def replace_root_summary(text, block):
    require(text.count(ROOT_SUMMARY_START) == text.count(ROOT_SUMMARY_END) == 1, "expected exactly one current snapshot block")
    before, rest = text.split(ROOT_SUMMARY_START); _, after = rest.split(ROOT_SUMMARY_END)
    return before + block + after


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--check", action="store_true"); p.add_argument("--validate", action="store_true")
    p.add_argument("--snapshot", metavar="REVISION", help="select revisions/<40-hex-sha> receipt data")
    p.add_argument("--root-summary", action="store_true", help="explicitly update or check the root README current-snapshot block")
    args = p.parse_args()
    if args.root_summary and not args.snapshot: p.error("--root-summary requires --snapshot")
    if args.root_summary and args.validate: p.error("--root-summary cannot be combined with --validate")
    if args.snapshot == '8571ca2dbecabf8d7863c369dc776611fd57d8ad':
        p.error('857 is frozen; verify it only at public checkout d0b396559a8ffda965cb495063b2967847d116aa (see benchmarks/compatibility/README.md)')
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
        if args.root_summary:
            path = ROOT / "README.md"; old = path.read_text(); new = replace_root_summary(old, render_root_summary(fs))
            if args.check: require(old == new, "stale current snapshot summary: README.md")
            else: path.write_text(new)
        return
    for path, view in ((ROOT / "README.md", "readme"), (ROOT / "docs/stdlib.md", "stdlib")):
        old = path.read_text(); new = replace_block(old, render(view))
        if args.check: require(old == new, f"stale scorecard: {path.relative_to(ROOT)}")
        else: path.write_text(new)


if __name__ == "__main__": main()
