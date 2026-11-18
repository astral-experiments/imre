"""Inactive, script-specific Ruff-06 source-as-data control gate."""

import ast
import hashlib
import json
import re
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
import raw_control
import scorecard

require = scorecard.require
POLICY_SHA = {
    "original": "4326bd79af09318b76675c8a76cd43f3a84c96bf609b874fb1b8fdc8b7150547",
    "invalid-control": "8079614fddc9867ef10921ead22bdd4ee54493f8ca8589367f52b68c696f5d1c",
}
CUSTODY_SHA = "b45e69d48ff1c36d006291b6060551a369824af4cebf2b40dacb0cf3d42d0d84"
WORK = "/dev/shm/ruff06-selective-959-work"
SITE = "/workspace/ruff-a-cache/deps/checkscripts-venv/lib/python3.14/site-packages"
IMAGE = "/workspace/scratch/ruff06-controls-959/build/ruff06-"
ENTRY = "scripts/check-scripts.py"
WITHDRAWN_SHA = "442305e8b0ad1dea10f57b8aa06245e77112d4711c082ed97ca1eeaf4c6ea3d0"


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def data(path):
    return json.loads(Path(path).read_text())


def bound(root, path, digest):
    p = (root / path).resolve()
    require(
        p.is_relative_to(root.resolve()) and p.is_file() and (sha(p) == digest),
        "mutable artifact digest or path differs",
    )
    return p


def authority(root=HERE):
    require(
        sha(root / "source/custody.json") == CUSTODY_SHA,
        "selective source authority differs",
    )
    custody = data(root / "source/custody.json")
    before_path = root / "source/stock/before.json"
    require(
        sha(before_path) == custody["raw_stock"]["stock/before.json"]["sha256"],
        "physical83 before authority differs",
    )
    before = data(before_path)
    require(
        before == {x["path"]: x["sha256"] for x in custody["fixture"]}
        and all(
            sha(root / "source/fixture" / x["path"]) == x["sha256"]
            for x in custody["fixture"]
        ),
        "approved fixture path hash changed against source authority",
    )
    return before


def validate(tag, binding=None, root=HERE):
    binding = binding or data(root / "binding.json")
    tier, variant = tag.split("-", 1)
    require(
        tier in ("O0", "O3") and variant in ("original", "invalid", "withdrawn"),
        "unknown variant",
    )
    name = "invalid-control" if variant == "invalid" else "original"
    role = binding["runs"][tag]
    require(
        role["policy_variant"] == name
        and binding["policy_sha256"][name] == POLICY_SHA[name],
        "variant cannot substitute for original policy",
    )
    p = root / ("policy-" + name + ".json")
    require(sha(p) == POLICY_SHA[name], "immutable policy authority differs")
    policy = data(p)
    orig = data(root / "policy-original.json")
    require(
        sha(root / "policy-original.json") == POLICY_SHA["original"],
        "original policy authority differs",
    )
    before = authority(root)
    reads = orig["fixture_reads"]
    require(
        set(reads) == {".gitignore", "tools/rotate.py", ENTRY}
        and reads == {k: before[k] for k in reads},
        "fixture read authority differs",
    )
    source = (root / "source/fixture" / ENTRY).read_bytes().splitlines(keepends=True)
    for x in orig["witnesses"]:
        require(
            hashlib.sha256(b"".join(source[x["first"] - 1 : x["last"]])).hexdigest()
            == x["sha256"],
            "upstream source witness differs",
        )
    if variant == "invalid":
        require(
            policy["original_policy_sha256"] == POLICY_SHA["original"]
            and sha(root / "evidence/invalid-entry.py")
            == policy["fixture_reads"][ENTRY]
            and (
                policy["fixture_reads"]
                == {**reads, ENTRY: sha(root / "evidence/invalid-entry.py")}
            ),
            "invalid control source authority differs",
        )
    run = data(bound(root, role["run"], role["run_sha256"]))
    trace = bound(root, role["trace"], role["trace_sha256"])
    require(
        run["trace_sha256"] == role["trace_sha256"] and trace.name == run["trace_file"],
        "mutable trace binding differs",
    )
    image = IMAGE + tier
    require(
        run["trace_argv"]
        == [
            "/usr/bin/strace",
            "-f",
            "-yy",
            "-s",
            "4096",
            "-o",
            "/workspace/scratch/ruff06-controls-959/recapture/" + tag + ".trace",
            "--",
            *run["argv"],
        ],
        "full strace coverage or capture command differs",
    )
    require(
        run["tag"] == tag
        and run["variant"] == variant
        and (run["cwd"] == WORK)
        and (run["environment"].get("PYTHONPATH") == SITE)
        and (run["argv"] == [image, "--write-ty-args-file", "scripts.txt"]),
        "native command or cwd differs",
    )
    build = data(root / "evidence" / ("build-" + tier + ".json"))
    require(
        build["image"] == image
        and build["image_sha256"] == policy["image_sha256"][tier]
        and (build["exit_code"] == 0),
        "saved image binding differs",
    )
    expected = before.copy()
    if variant == "invalid":
        expected[ENTRY] = policy["fixture_reads"][ENTRY]
    if variant == "withdrawn":
        del expected[ENTRY]
    require(
        run["original_preimage"] == before and run["before"] == expected,
        "approved fixture path hash changed against source authority",
    )
    output = WITHDRAWN_SHA if variant == "withdrawn" else orig["output"]["sha256"]
    require(
        run["exit_code"] == 0
        and run["effects"] == {"scripts.txt": [None, output]}
        and (run["after"] == {**expected, "scripts.txt": output}),
        "native effect differs",
    )
    require(
        sha(root / "evidence" / (tag + ".scripts.txt")) == output
        and sha(root / "evidence" / run["stdout_file"])
        == run["stdout_sha256"]
        == hashlib.sha256(b"").hexdigest()
        and (
            sha(root / "evidence" / run["stderr_file"])
            == run["stderr_sha256"]
            == hashlib.sha256(b"").hexdigest()
        ),
        "native raw output differs",
    )
    text = trace.read_text()
    lines = text.splitlines()
    calls = [line for line in lines if re.search("\\bexecve(?:at)?\\(", line)]
    match = raw_control.EXEC.fullmatch(calls[0]) if len(calls) == 1 else None
    require(
        match is not None
        and json.loads(match[2]) == image
        and (json.loads(match[3]) == run["argv"]),
        "unapproved exec or child",
    )
    pid = match[1]
    require(
        not any(
            re.search(
                "\\b(?:chdir|fchdir|dup|dup2|dup3|clone|clone3|fork|vfork|io_uring_setup|memfd_create|pidfd_getfd)\\(",
                line,
            )
            or re.search("\\bfcntl\\(.*F_DUPFD", line)
            for line in lines
        ),
        "cwd/fd alias or native child operation denied",
    )

    def norm(s):
        return (
            s.replace("{cwd}", WORK)
            .replace("{site}", SITE)
            .replace("{image}", image)
            .replace("{pid}", pid)
            if s is not None
            else None
        )

    expected_events = Counter()
    for x in policy["native_open_census"]:
        if variant == "withdrawn" and x["lexical"] == "{cwd}/" + ENTRY:
            continue
        expected_events[
            x["operation"],
            norm(x["lexical"]),
            norm(x["kernel"]),
            x["flags"],
            x["mode"],
            norm(x["directory_witness"]),
        ] += x["count"]
    observed = Counter()
    for event in scorecard.successful_open_paths(text, WORK, kernel_events=True):
        lex = event["lexical"]
        path = str(event["path"])
        returned = str(event["returned"])
        line = lines[event["line"] - 1]
        require(
            event["pid"] == pid
            and event["operation"] in ("open", "openat")
            and (".." not in Path(lex).parts)
            and (
                not any(
                    raw_control.payload_alias(v, task=True)
                    for v in (lex, path, returned)
                )
            ),
            "unapproved alias/open operation",
        )
        require(
            event["operation"] != "openat"
            or re.search("\\bopenat\\(AT_FDCWD<" + re.escape(WORK) + ">,", line),
            "unapproved dirfd/cwd alias",
        )
        key = (
            event["operation"],
            lex,
            returned,
            event["flags"],
            event["mode"],
            event["directory_witness"],
        )
        require(
            key in expected_events, "unauthorized native content open or flags: " + lex
        )
        observed[key] += 1
    require(observed == expected_events, "native content census differs")
    fixture_reads = {
        WORK + "/" + k: v
        for k, v in policy["fixture_reads"].items()
        if not (variant == "withdrawn" and k == ENTRY)
    }
    read_hashes = {k: [] for k in fixture_reads}
    for line in lines:
        m = re.fullmatch(
            '(\\d+) read\\(\\d+<([^<>]+)>, ("(?:\\\\.|[^"\\\\])*"), \\d+\\) = (\\d+)',
            line,
        )
        if m and m[2] in fixture_reads and int(m[4]):
            require(m[1] == pid, "fixture read PID differs")
            try:
                payload = ast.literal_eval("b" + m[3])
            except (SyntaxError, ValueError):
                require(False, "fixture read bytes undecodable")
            require(len(payload) == int(m[4]), "fixture read was truncated")
            read_hashes[m[2]].append(hashlib.sha256(payload).hexdigest())
    require(
        all((read_hashes[k] == [v] for k, v in fixture_reads.items())),
        "fixture read bytes differ from immutable source authority",
    )
    return run
