"""Append-only public authority for a completed finite original-test cohort."""

import json
import subprocess
from functools import lru_cache

import raw_control
import source_stage_pins
from extensions.data_resource959_v1 import authority as resource
from extensions.successor959_v1 import authority as common

BASE = "benchmarks/compatibility/extensions/completed_unittest959_activation_v1"
MANIFEST = f"{BASE}/manifest.json"
SOURCE = {
    "commit": "656141bd7cb572ffbaebf981d6bd385e99c9f599",
    "manifest": "benchmarks/compatibility/extensions/completed_unittest959_v1/manifest.json",
    "git_blob": "5e5aac64ab74e357ac393cf3cec5042ed38d411c",
    "sha256": "c66861317b95f4640e91d5e0134db308a7c3a955d30a59dd7d074b4cc87f28fa",
}
TARGETS = {
    f"benchmarks/compatibility/{path}"
    for path in (
        "scorecard.py",
        "raw_control.py",
        "extensions/successor959_v1/authority.py",
        "extensions/data_resource959_v1/authority.py",
    )
}
safe = common.safe_path
predecessor_stock = common.predecessor_stock


def fixed_pin(api):
    pin = getattr(source_stage_pins, "REVISION_959_COMPLETED", None)
    api.require(
        isinstance(pin, dict)
        and set(pin) == {"commit", "manifest", "git_blob", "sha256"}
        and pin["manifest"] == MANIFEST
        and api.HEX40.fullmatch(pin["commit"])
        and api.HEX40.fullmatch(pin["git_blob"])
        and api.HEX64.fullmatch(pin["sha256"])
        and pin["commit"] != SOURCE["commit"],
        "completed amendment requires a distinct public source pin",
    )
    return pin


def _public_source(api):
    raw_control.public_commit(api, SOURCE["commit"])
    source = raw_control.git_manifest(
        api.ROOT, SOURCE["commit"], SOURCE["manifest"], SOURCE["git_blob"], SOURCE["sha256"]
    )
    public_bytes = raw_control.git(
        api.ROOT, "show", "--no-textconv", f"{SOURCE['commit']}:{SOURCE['manifest']}"
    )
    api.require(
        source["credit"] == 0 and safe(api, SOURCE["manifest"]).read_bytes() == public_bytes,
        "completed public source manifest differs",
    )
    for path, digest in source["files"].items():
        api.require(
            raw_control.sha(safe(api, path).read_bytes()) == digest,
            f"completed source evidence changed: {path}",
        )
    return source


def _verify_files(api, files):
    extension = api.ROOT / BASE
    excluded = {extension / "manifest.json", extension / "activation.json"}
    actual = {
        path.relative_to(api.ROOT).as_posix()
        for path in extension.rglob("*")
        if path.is_file() and path not in excluded
    }
    api.require(actual == set(files), "completed amendment file closure differs")
    api.require(
        not any(path.is_symlink() for path in extension.rglob("*")),
        "completed amendment contains a symlink",
    )
    for path, digest in files.items():
        api.require(
            path.startswith(f"{BASE}/")
            and raw_control.sha(safe(api, path).read_bytes()) == digest,
            f"completed amendment file digest differs: {path}",
        )


def _verify_patch(api, patch, files, post):
    api.require(
        patch["path"] in files and files[patch["path"]] == patch["sha256"],
        "completed amendment patch differs",
    )
    stat = subprocess.check_output(
        ["git", "apply", "--numstat", str(safe(api, patch["path"]))], cwd=api.ROOT, text=True
    )
    observed = [line.split("\t", 2)[2] for line in stat.splitlines()]
    items = patch["files"]
    api.require(
        len(observed) == len(set(observed)) == len(items) == len(TARGETS)
        and set(observed) == {item["path"] for item in items} == TARGETS,
        "completed amendment patch target closure differs",
    )
    changes = {}
    for item in items:
        path = item["path"]
        revision_path = f"{SOURCE['commit']}:{path}"
        original = raw_control.git(api.ROOT, "show", "--no-textconv", revision_path)
        blob = raw_control.git(api.ROOT, "rev-parse", revision_path).decode().strip()
        api.require(
            item["old_git_blob"] == blob and item["old_sha256"] == raw_control.sha(original),
            f"completed amendment source preimage differs: {path}",
        )
        api.require(api.HEX64.fullmatch(item["post_sha256"]), "invalid completed source postimage")
        expected = item["post_sha256"] if post else item["old_sha256"]
        api.require(
            raw_control.sha(safe(api, path).read_bytes()) == expected,
            f"completed amendment source postimage differs: {path}",
        )
        changes[path] = item
    return changes


def _verify_binding(api, name, entry, source, files, allowed_files):
    api.require(
        set(entry) == {"adapter", "binding", "receipt", "staged_receipt"}
        and entry["adapter"] == "raw-control-v4"
        and entry["binding"] in files
        and entry["staged_receipt"] == source["evidence"]["receipt"]
        and entry["receipt"] == (
            f"benchmarks/compatibility/revisions/{source['revision']}/"
            f"verified/completed-finite-v1/stdlib/{name}.jsonl"
        ),
        "completed amendment binding differs",
    )
    binding = json.loads(safe(api, entry["binding"]).read_bytes())
    staged_bytes = safe(api, entry["staged_receipt"]).read_bytes()
    rows = [json.loads(line) for line in staged_bytes.splitlines()]
    keys = [(row["id"], row["occurrence"]) for row in binding["originals"]]
    denominator = binding["canonical_denominator"]
    api.require(
        binding["family"] == name
        and binding["source"] == source["source"]
        and denominator == api.families()[name]["originals"],
        "completed binding source or canonical denominator differs",
    )
    api.require(
        0 < len(keys) == binding["observed_originals"] <= denominator
        and binding["other_originals_not_run"] == denominator - len(keys)
        and len(keys) == len(set(keys)),
        "completed canonical original subset differs",
    )
    api.require(
        keys == [(row["id"], row["occurrence"]) for row in rows]
        and all(not row["paired"] and row["source"] == binding["source"] for row in rows),
        "completed staged original order, source or zero credit differs",
    )
    for row in binding["originals"]:
        api.canonical_original(
            {
                "family": name, "id": row["id"], "occurrence": row["occurrence"],
                "source": binding["source"],
            },
            "stdlib",
        )
    for archive, seal in binding["archives"].items():
        api.require(
            archive in allowed_files
            and seal["outer_seal"] in allowed_files
            and allowed_files[archive] == seal["archive_sha256"]
            and allowed_files[seal["outer_seal"]] == seal["outer_sha256"],
            "completed archive or seal is outside the file closure",
        )
    for locator in binding["roles"].values():
        archive, separator, member = locator.partition("!")
        api.require(
            locator in allowed_files
            or (separator and archive in binding["archives"] and bool(member)),
            "completed artifact role closure differs",
        )


def verify_stage(api, manifest, post=True):
    api.require(
        manifest.get("schema") == "imre.compatibility.completed-finite-amendment.v1"
        and manifest.get("credit") == 0
        and manifest.get("source_stage") == SOURCE,
        "completed amendment source or credit differs",
    )
    source = _public_source(api)
    files = manifest["files"]
    _verify_files(api, files)
    changes = _verify_patch(api, manifest["patch"], files, post)
    allowed_files = {**source["files"], **files}
    for name, entry in manifest["bindings"].items():
        _verify_binding(api, name, entry, source, files, allowed_files)
    return manifest, source, changes


@lru_cache(maxsize=None)
def _stage(api, pin_json):
    pin = json.loads(pin_json)
    raw_control.public_commit(api, pin["commit"])
    manifest = raw_control.git_manifest(
        api.ROOT, pin["commit"], pin["manifest"], pin["git_blob"], pin["sha256"]
    )
    api.require(
        raw_control.sha(safe(api, pin["manifest"]).read_bytes()) == pin["sha256"],
        "completed local manifest differs from public stage",
    )
    return verify_stage(api, manifest)


def stage(api):
    return _stage(api, json.dumps(fixed_pin(api), sort_keys=True))


def expected(api, path, digest):
    _, _, changes = stage(api)
    change = changes.get(path)
    api.require(
        change is None or change["old_sha256"] == digest,
        "completed amendment predecessor source conflicts",
    )
    return change["post_sha256"] if change else digest


def _preserved_original(api, staged, activated):
    mutable_row_fields = {"paired", "credit_reason", "runs"}
    mutable_run_fields = {"argv", "evidence"}
    unchanged_row = {key: value for key, value in staged.items() if key not in mutable_row_fields}
    activated_row = {
        key: value for key, value in activated.items() if key not in mutable_row_fields
    }
    if unchanged_row != activated_row:
        return False
    return all(
        {
            key: value for key, value in staged["runs"][tier].items()
            if key not in mutable_run_fields
        } == {
            key: value for key, value in activated["runs"][tier].items()
            if key not in mutable_run_fields
        }
        for tier in api.TIERS
    )


@lru_cache(maxsize=None)
def activated_entries(api):
    manifest, _, _ = stage(api)
    activation = json.loads(safe(api, f"{BASE}/activation.json").read_bytes())
    api.require(
        set(activation) == {"schema", "source_stage", "families", "receipt_hashes"}
        and activation["schema"] == "imre.compatibility.completed-finite-activation.v1"
        and activation["source_stage"] == fixed_pin(api)
        and activation["families"] == manifest["bindings"]
        and set(activation["receipt_hashes"]) == {
            entry["receipt"] for entry in activation["families"].values()
        },
        "completed activation differs from public amendment",
    )
    for entry in activation["families"].values():
        receipt = safe(api, entry["receipt"]).read_bytes()
        api.require(
            raw_control.sha(receipt) == activation["receipt_hashes"][entry["receipt"]],
            "completed receipt hash differs",
        )
        staged_bytes = safe(api, entry["staged_receipt"]).read_bytes()
        staged = [json.loads(line) for line in staged_bytes.splitlines()]
        activated = [json.loads(line) for line in receipt.splitlines()]
        api.require(
            len(staged) == len(activated)
            and all(_preserved_original(api, old, new) for old, new in zip(staged, activated)),
            "completed activation altered a frozen original outcome or reason",
        )
    return activation


def effective_binding(api, entry):
    return json.loads(safe(api, entry["binding"]).read_bytes())


def read(api, entry, locator):
    manifest, source, _ = stage(api)
    api.require(entry in manifest["bindings"].values(), "completed entry was not staged")
    binding = effective_binding(api, entry)
    allowed = set(binding["roles"].values()) | {
        entry["binding"], entry["staged_receipt"], entry["receipt"],
    }
    api.require(locator in allowed, "unbound completed artifact")
    archive, separator, member = locator.partition("!")
    if separator:
        api.require(archive in binding["archives"], "unknown completed archive")
        config = json.dumps(binding["archives"][archive], sort_keys=True)
        members = common.archive_members(api, archive, config)
        api.require(member in members, "unbound completed member")
        return members[member]

    if locator == entry["receipt"]:
        digest = activated_entries(api)["receipt_hashes"][locator]
    else:
        digest = {**source["files"], **manifest["files"]}[locator]
    content = safe(api, locator).read_bytes()
    api.require(raw_control.sha(content) == digest, "completed artifact digest differs")
    return content


def digest(api, entry, locator):
    return raw_control.sha(read(api, entry, locator))


def artifact(api, entry, locator):
    content = read(api, entry, locator)
    path = safe(api, locator)
    api.require(path.read_bytes() == content, "completed artifact differs")
    return path


def donor(api, product):
    _, source, _ = stage(api)
    entries = resource.activated_entries(api)["families"]
    donor_binding = source["custody"]["public_donor"]
    matches = [entry for entry in entries.values() if entry["binding"] == donor_binding]
    api.require(len(matches) == 1, "completed source donor is not uniquely public")
    resource.donor(api, matches[0], product)
