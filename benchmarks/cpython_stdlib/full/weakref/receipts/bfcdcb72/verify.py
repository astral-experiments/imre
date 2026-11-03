"""Verify the sealed runs offline with `python3 -I -B verify.py`.

To repeat in a Linux shell, set IMRE and CPYTHON to clean checkouts of the
revisions below, STOCK to the absolute path to that CPython build's python,
and RECEIPT to this directory. Use rustc/cargo 1.98.1 (rust-toolchain.toml)
and Clang 18, LLD 18 and Linux native link inputs, as in the Imre Linux release.

    set -eu
    test "$(git -C "$IMRE" rev-parse HEAD)" = bfcdcb727b9b00d8da4697a02b389e868d82e3da
    pin=5607950ef232dad16d75c0cf53101d9649d89115
    test "$(git -C "$CPYTHON" rev-parse HEAD)" = "$pin"
    W=$(mktemp -d)
    tar -xzf "$RECEIPT/raw.tar.gz" -C "$W" harness/checkpoint_test_weakref.py harness/finalize_parent.py
    export TMPDIR="$W/tmp" XDG_CACHE_HOME="$W/cache"
    mkdir "$TMPDIR" "$XDG_CACHE_HOME"
    unset PYTHONHOME PYTHONPATH PYTHONUNBUFFERED IMREFLAGS IMRE_ENCODED_FLAGS
    "$STOCK" -I -c 'import sys; assert sys.version_info[:3] == (3, 14, 5)'
    export WEAKREF_CHECKPOINT_KEYS=test.test_weakref.FinalizeTestCase.test_atexit#1
    (cd "$W/harness" && "$STOCK" -B -m checkpoint_test_weakref)
    for opt in 0 3; do
        base="$W/O$opt"
        target="$base/target"
        prefix="$base/prefix"
        mkdir -p "$prefix/bin" "$prefix/lib/imre3.14/site-packages/test"
        (cd "$IMRE" && IMRE_DEFAULT_NATIVE_OPT_LEVEL="$opt" CARGO_TARGET_DIR="$target" \
            CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
            cargo build --locked -p imre-runtime -p imre-compiler)
        cp "$target/debug/imre" "$target/debug/libimre_runtime.a" "$prefix/bin/"
        git -C "$CPYTHON" archive "$pin":Lib/test | tar -x -C "$prefix/lib/imre3.14/site-packages/test"
        (cd "$W/harness" && "$prefix/bin/imre" -B -m finalize_parent ordinary)
    done

The ordinary parent uses the real script_helper. A separate observed-parent
run records that helper's returned child streams. The isolated child replay
and the retained/released owner controls are diagnostics outside the count.
"""

import ast
import base64
import hashlib
import json
from pathlib import Path
import tarfile

IMRE = "bfcdcb727b9b00d8da4697a02b389e868d82e3da"
CPYTHON = "5607950ef232dad16d75c0cf53101d9649d89115"
TEST = "test.test_weakref.FinalizeTestCase.test_atexit"
PROG = "from test.test_weakref import FinalizeTestCase;FinalizeTestCase.run_in_child()"
CHILD = ["-X", "faulthandler", "-I", "-c", PROG]
OUT = b"f4 foobar\nf3 error\ng1\nf1 foobar\n"


def check(ok, label):
    if not ok:
        raise ValueError(label)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_run(raw, level, name, exit_code=0):
    stem = f"{level}/{name}"
    record = json.loads(raw[stem + ".json"])
    check(record["exit"] == exit_code, stem + " process exit")
    stdout, stderr = (raw[stem + "." + stream] for stream in ("stdout", "stderr"))
    check(digest(stdout) == record["stdout_sha256"] and digest(stderr) == record["stderr_sha256"], stem + " streams")
    return record, stdout, stderr


def parent_events(stdout):
    prefixes = ("REGISTERED_COUNT", "startTest", "add", "stopTest", "RUNNER_RETURN")
    check([line for line in stdout.decode().splitlines() if line.startswith(prefixes)] == [
        "REGISTERED_COUNT 1", "startTest " + TEST, "addSuccess " + TEST,
        "stopTest " + TEST, "RUNNER_RETURN 1 True",
    ], "ordinary parent callbacks or teardown")


def observed_child(level, stdout):
    start = stdout.index("startTest " + TEST + "\n")
    end = stdout.index("addSuccess " + TEST + "\n")
    stdout = stdout[start:end]
    if level == "stock":
        entry, = [json.loads(line.removeprefix("ACTUAL_CHILD ")) for line in stdout.splitlines()
                  if line.startswith("ACTUAL_CHILD ")]
        return entry["argv"], entry["exit"], base64.b64decode(entry["stdout_b64"]), base64.b64decode(entry["stderr_b64"])
    fields = [line.split(" ", 1) for line in stdout.splitlines() if line.startswith("NESTED_")]
    check([name for name, _ in fields] == ["NESTED_ARGV", "NESTED_RC", "NESTED_OUT", "NESTED_ERR"],
          level + " observed helper return")
    return tuple(ast.literal_eval(value) for _, value in fields)


def verify_native(raw, level, driver, ordinary, replay):
    build = json.loads(raw[level + "/compiler-build.json"])
    check(build["revision"] == IMRE and build["clean"] and build["cargo_exit"] == 0, level + " clean build")
    check(build["build_environment"]["IMRE_DEFAULT_NATIVE_OPT_LEVEL"] == int(level[1]), level + " build default")
    check(build["compiler"] == driver and ordinary["exe_sha256"] == build["compiler_sha256"], level + " driver identity")
    cargo = raw[level + "/cargo-excerpt.txt"].decode()
    for line in (f"source {IMRE} tree {build['tree']}", "Compiling imre-compiler ", "Finished `dev` profile",
                 f"IMRE_DEFAULT_NATIVE_OPT_LEVEL={level[1]}", build["compiler_sha256"], build["runtime_sha256"]):
        check(line in cargo, level + " Cargo build record")

    def events(filename):
        return [json.loads(line) for line in raw[f"{level}/{filename}"].splitlines()]

    def check_backend(row):
        inputs = row["inputs_sha256"]
        ir, = [path for path in inputs if path.endswith(".ll")]
        check(row["exit"] == 0 and inputs[build["runtime"]] == build["runtime_sha256"], level + " backend exit/runtime")
        check(row["args"][0] == "-" + level and ir in row["args"] and len(inputs[ir]) == 64, level + " LLVM level")
        check(len(row["output_sha256"]) == 64 and row["output"].endswith("/program"), level + " ELF")

    ordinary_trace = events("parent.clang.jsonl")
    top, = [r for r in ordinary_trace if r["parents"][0]["argv"] == ordinary["argv"]]
    check_backend(top)
    check(replay["exe_sha256"] == top["output_sha256"] and replay["argv"] == [top["retained"], "observed"],
          level + " observed-parent ELF")
    transitions = {(r["pid"], r["ppid"], r["start_ticks"]) for r in events("proc-transitions.jsonl")}
    for label, trace in (("ordinary", ordinary_trace), ("observed", events("observed-parent.clang.jsonl"))):
        child, = [r for r in trace if r["parents"][0]["argv"] == [driver, *CHILD]]
        check_backend(child)
        caller, parent = child["parents"]
        for proc in (caller, parent):
            check((proc["pid"], proc["ppid"], proc["start_ticks"]) in transitions, level + " process lifetime")
        check(caller["pid"] != parent["pid"] and caller["ppid"] == parent["pid"] and caller["exe"] == driver,
              level + " isolated child ancestry")
        if label == "ordinary":
            check(parent["argv"] == ["-m", label] and parent["pid"] == top["parents"][0]["pid"],
                  level + " ordinary parent")
        else:
            check(parent["argv"] == replay["argv"] and parent["exe"] == replay["argv"][0], level + " observed parent")
        print(f"{level} {label} parent PID {parent['pid']}, isolated child PID {caller['pid']}")
    missing, out, err = read_run(raw, level, "no-package", 1)
    check(missing["exe_sha256"] == ordinary["exe_sha256"] and missing["runtime_sha256"] == build["runtime_sha256"]
          and missing["argv"][1:] == CHILD and not out and b'cannot resolve first-party module "test"' in err,
          level + " empty-install control")


def verify(raw):
    install = json.loads(raw["install.json"])
    check(install["cpython_revision"] == CPYTHON and install["files"] == 1639, "installed CPython package")
    check(install["source_tarball_sha256"] == "0d838be32b52da92a4293ecd99bc91f7be8b4acc573423665bd1a1428be46e47",
          "pinned CPython source archive")
    check(install["upstream_tree_sha256"] == "5852e025ab04596bbff24c13131ba5dcbe3d0d0077483a0a26a56bc5c8eb8504",
          "pinned Lib/test inventory")
    check(all(value == install["upstream_tree_sha256"] for value in install["installed_tree_sha256"].values())
          and set(install["installed_tree_sha256"]) == {"O0", "O3"}, "installed tree differs from pin")
    staged = Path(__file__).resolve().parents[4]
    sources = [
        ("upstream/Lib/test/test_weakref.py", "test_weakref_sha256", "dfb1373b1878b79f964582693cc83dfb734d6f43c0c1dee5d518b9e266792ea7"),
        ("full/shared/upstream/Lib/test/support/script_helper.py", "script_helper_sha256", "d94c1502c2b7a3f1e57deb1c8913e1890049c73979261f205fc2f7e96fc03677"),
    ]
    for path, field, pin in sources:
        check(digest((staged / path).read_bytes()) == install[field] == pin, "source drift: " + path)
    check(json.loads(raw["stock/registration.json"]) == ["register", 3, TEST, 1], "stock occurrence")
    for level in ("stock", "O0", "O3"):
        ordinary, stdout, _ = read_run(raw, level, "parent")
        parent_events(stdout)
        driver = ordinary["argv"][0]
        replay, observed, _ = read_run(raw, level, "observed-parent")
        parent_events(observed)
        argv, rc, actual_out, actual_err = observed_child(level, observed.decode())
        check(argv == [driver, *CHILD] and rc == 0 and actual_out == OUT and b"ZeroDivisionError" in actual_err,
              level + " observed nested child")
        direct, out, err = read_run(raw, level, "isolated")
        check(direct["argv"] == argv and out == actual_out and err == actual_err, level + " separate isolated replay")
        merged, combined, separate = read_run(raw, level, "isolated-combined")
        check(merged["merged_fd"] and not separate and combined == err + out, level + " combined pipe replay")
        if level == "stock":
            check(ordinary["exe_sha256"] == install["stock_executable_sha256"], "stock interpreter")
        else:
            check(ordinary["impl_revision"] == IMRE and ordinary["stock_pin"] == CPYTHON
                  and ordinary["pinned_test_sha256"] == install["test_weakref_sha256"], level + " source pin")
            expected_file = Path(driver).parents[1] / "lib/imre3.14/site-packages/test/test_weakref.py"
            check(f"ORIGINAL_SOURCE {expected_file} ".encode() in stdout, level + " parent import")
            verify_native(raw, level, driver, ordinary, replay)
        for label in ("retained", "released"):
            checker = json.loads(raw[f"{level}/control-{label}-checker.json"])
            check(checker["exit"] == int(label == "released"), level + " retained-owner predicate")
            _, out, err = read_run(raw, level, "control-" + label)
            check(out == raw["stock/control-" + label + ".stdout"] and b"ZeroDivisionError" in err,
                  level + " " + label + " owner control")
    retained = raw["stock/control-retained.stdout"].decode().splitlines()
    released = raw["stock/control-released.stdout"].decode().splitlines()
    check(retained[1] == "BODY_END retained owners 1 owner_alive True events []"
          and retained[2:] == ["CALL " + name + " owners 1 owner_alive True" for name in ("last", "error", "new", "old")],
          "owner retained through shutdown")
    check(released[1:5] == ["CALL " + name + " owners 0 owner_alive False" for name in ("last", "error", "disabled", "old")]
          and released[5].startswith("BODY_END released owners 0 owner_alive False") and released[6:] == ["CALL new owners 0 owner_alive False"],
          "last owner released before shutdown")
    print(TEST + "#1: stock, O0 and O3 passed")


if __name__ == "__main__":
    with tarfile.open(Path(__file__).with_name("raw.tar.gz")) as archive:
        verify({f.name: archive.extractfile(f).read() for f in archive if f.isfile()})
