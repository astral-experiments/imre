"""Offline validation of the inactive, script-specific Ruff-06 source stage."""

import hashlib
import json
import subprocess

import policy_gate as gate

H = gate.HERE
E = H / "evidence"
S = H / "source"
check = gate.require
sha = gate.sha
load = gate.data


def main():
    c = load(S / "custody.json")
    b = load(H / "historical-binding.json")
    stage = load(H / "stage.json")
    before = gate.authority()
    check(sha(H / "binding.json") == stage["binding_sha256"], "stage binding differs")
    check(
        stage["policy_original_sha256"] == gate.POLICY_SHA["original"]
        and stage["policy_invalid_control_sha256"]
        == gate.POLICY_SHA["invalid-control"],
        "stage policy authority differs",
    )
    check(
        len([p for p in S.rglob("*") if p.is_file()]) == 12,
        "selective bundle footprint",
    )
    check(
        before == {x["path"]: x["sha256"] for x in c["fixture"]},
        "physical83 before-state differs",
    )
    for x in c["fixture"]:
        check(
            sha(S / "fixture" / x["path"]) == x["sha256"]
            and (S / "fixture" / x["path"]).stat().st_size == x["bytes"],
            "fixture bytes differ",
        )
    for n, x in c["raw_stock"].items():
        check(
            sha(S / n) == x["sha256"] and (S / n).stat().st_size == x["bytes"],
            "later raw bytes differ",
        )
    a = subprocess.check_output(
        ["git", "-C", str(H), "show", b["authority_commit"] + ":" + b["authority_path"]]
    )
    line = next(x for x in a.splitlines() if json.loads(x).get("id") == "ruff-06")
    original = json.loads(line)
    check(
        hashlib.sha256(line).hexdigest() == b["authority_record_sha256"]
        and original["source"] == b["source"]
        and original["original_argv"] == b["original_argv"],
        "historical original authority differs",
    )
    check(
        b["source"]["commit"] == "65aee8892ce379af4ae9c8b5f6e0ab37b25d5ec9"
        and b["source"]["sha256"] == before[gate.ENTRY],
        "Ruff public pin differs",
    )
    entry = (S / "fixture" / gate.ENTRY).read_bytes()
    check(
        hashlib.sha1(b"blob " + str(len(entry)).encode() + b"\0" + entry).hexdigest()
        == b["source"]["git_blob"],
        "Ruff Git blob differs",
    )
    check(
        b["later_stock_is_historical"] is False
        and b["later_stock_observation_date"] is None
        and b["archive_preexistence_claim"] is False
        and stage["stock_observation_kind"] == "LATER"
        and stage["activation"] is False
        and stage["credit"] == 0,
        "provenance or activation labels differ",
    )
    wait = load(S / "stock/cli.wait.json")
    check(
        wait["pid"] > 0
        and wait["raw_wait"] == wait["returncode"] == 0
        and wait["stdout_sha256"] == sha(S / "stock/cli.stdout")
        and wait["stderr_sha256"] == sha(S / "stock/cli.stderr"),
        "later stock PID/wait differs",
    )
    ref = load(S / "stock/effects.json")
    check(
        ref
        == {
            "scripts.txt": [
                None,
                "a952e456eb11d368f8f3030625fbab8e5bc7b5bfe790aec17cbb11aacbc2db2b",
            ]
        },
        "later full effect differs",
    )
    for tag in ("stock", "cli-O0", "cli-O3"):
        x = load(E / (tag + ".json"))
        check(
            x["before"] == before
            and x["effects"] == ref
            and x["exit_code"] == 0
            and x["matches_later_streams"]
            and x["observed_root_order"] == stage["fixture_root_traversal_order"]
            and sha(E / x["stdout_file"]) == x["stdout_sha256"]
            and sha(E / x["stderr_file"]) == x["stderr_sha256"]
            and sha(E / (tag + ".scripts.txt")) == ref["scripts.txt"][1],
            tag + " baseline differs",
        )
    invalid = load(E / "invalid-cpython.json")
    replacement = (E / "invalid-entry.py").read_bytes()
    check(
        invalid["argv"]
        == [
            "/workspace/ruff-a-cache/python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14",
            "-BS",
            gate.WORK + "/" + gate.ENTRY,
            "--write-ty-args-file",
            "scripts.txt",
        ]
        and b"# /// script" in replacement.splitlines()
        and invalid["exit_code"] == 1
        and invalid["effects"] == {}
        and invalid["before"][gate.ENTRY] == sha(E / "invalid-entry.py")
        and b"SyntaxError" in (E / invalid["stderr_file"]).read_bytes()
        and sha(E / invalid["stderr_file"]) == invalid["stderr_sha256"],
        "CPython invalid-source control differs",
    )
    custody = load(E / "image-custody.json")
    policy = load(H / "policy-original.json")
    check(
        custody["saved_image_sha256"] == policy["image_sha256"],
        "saved image custody differs",
    )
    for tier in ("O0", "O3"):
        for variant in ("original", "invalid", "withdrawn"):
            gate.validate(tier + "-" + variant)
    poison = load(E / "poison.json")
    check(
        len(poison) == 4
        and all(
            x["exit_code"] == 1
            and not x["image_exists"]
            and "expected int, got bool" in x["diagnostic"]
            for x in poison.values()
        ),
        "poison controls differ",
    )
    for name, digest in stage["evidence_sha256"].items():
        check(sha(H / name) == digest, "evidence binding differs: " + name)
    check(
        {p.name for p in E.glob("*.trace")}
        == {
            f"{t}-{v}.trace"
            for t in ("O0", "O3")
            for v in ("original", "invalid", "withdrawn")
        },
        "superseded trace or missing authoritative trace",
    )
    print(
        "Ruff-06 original/invalid/withdrawn policies and source custody verified; zero credit"
    )


if __name__ == "__main__":
    main()
