"""Build frozen Flask and compare the SQLite tracker against stock Flask over TCP."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import sqlite3
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
ROOT = BENCH.parents[1]
sys.path[:0] = [str(BENCH / "frozen"), str(BENCH), str(HERE)]
import gate
import native_identity
import prepare
import traffic
from storage.build import verify as verify_storage


def seed(path, env):
    with sqlite3.connect(path) as db:
        db.executescript(
            """
            CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE,
                               token_hash TEXT NOT NULL);
            CREATE TABLE issues(id INTEGER PRIMARY KEY, owner INTEGER NOT NULL REFERENCES users(id),
                                title TEXT NOT NULL, body TEXT NOT NULL,
                                status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open','closed')));
            CREATE INDEX issue_owner_status ON issues(owner, status, id);
        """
        )
        for name in ("alice", "bob"):
            token = env[f"TRACKER_{name.upper()}_TOKEN"]
            db.execute(
                "INSERT INTO users(name, token_hash) VALUES (?, ?)",
                (name, hashlib.sha256(token.encode()).hexdigest()),
            )


def python(script, site):
    paths = [str(site), str(HERE), str(HERE / "storage/cpython"), str(ROOT / "stdlib")]
    loader = (
        "import sys,runpy;sys.path[:0]=sys.argv[1:5];sys.argv=sys.argv[5:];"
        "runpy.run_path(sys.argv[0],run_name='__main__')"
    )
    return [sys.executable, "-ISB", "-c", loader, *paths, str(HERE / script)]


def attest(binary, db, env, server, work):
    trace = work / "native.strace"
    command = native_identity.traced_command(binary, trace)
    with traffic.serve(command, db, env, server) as (port, proc):
        native_identity.require_attached(proc.pid, trace, command[0])
        proof = native_identity.attest_socket(
            proc.pid, binary, port, route="/api/issues", status=401, body=b'{"error":401}\n'
        )
        client = traffic.Browser(port)
        client.login("native login", "alice", env["TRACKER_ALICE_TOKEN"])
        created = client.write(
            "native insert", "/api/issues", {"title": "Verify socket", "body": "trace"}, 201
        )
        client.request("native detail", "GET", f"/issues/{created['id']}")
        maps = Path(f"/proc/{proc.pid}/maps").read_text()
        if "libsqlite3.so" not in maps:
            raise RuntimeError("server did not map the SQLite library")
    audit = native_identity.audit_trace(trace, binary, proc.pid)
    return {"socket": proof, "trace": audit, "sqlite_mapped": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--work", type=Path, help="new directory for artifacts")
    parser.add_argument("--wheelhouse", type=Path, help="local copy of the seven pinned wheels")
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--server-cpu", type=int)
    parser.add_argument("--client-cpus", help="comma-separated CPU IDs")
    parser.add_argument("--imre", type=Path, help="use this already-built compiler and --runtime")
    parser.add_argument("--runtime", type=Path, help="matching static runtime archive")
    args = parser.parse_args()
    if (
        sys.platform != "linux"
        or sys.version_info[:3] != (3, 14, 5)
        or sys.implementation.name != "cpython"
        or sys.flags.optimize
    ):
        parser.error("run on Linux with CPython 3.14.5 and assertions enabled")
    if bool(args.imre) != bool(args.runtime):
        parser.error("--imre and --runtime must be supplied together")
    available = sorted(os.sched_getaffinity(0))
    server = args.server_cpu if args.server_cpu is not None else available[0]
    clients = (
        [int(n) for n in args.client_cpus.split(",")]
        if args.client_cpus
        else [n for n in available if n != server][:3]
    )
    if not clients or server in clients or not ({server} | set(clients)) <= set(available):
        parser.error("select distinct available CPUs for server and client")
    work = args.work.resolve() if args.work else Path(tempfile.mkdtemp(prefix="flask-tracker-"))
    if args.work:
        work.mkdir(parents=True)
    print(f"Artifacts: {work}", flush=True)
    if args.imre:
        compiler = args.imre.resolve(strict=True)
        runtime = args.runtime.resolve(strict=True)
    else:
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
        subprocess.run(
            [
                "cargo",
                "build",
                "--release",
                "--locked",
                "--target-dir",
                target,
                "-p",
                "imre-compiler",
                "-p",
                "imre-runtime",
            ],
            cwd=ROOT,
            check=True,
        )
        compiler = target / "release/imre"
        runtime = target / "release/libimre_runtime.a"
    site = prepare.source_tree(work, args.wheelhouse)
    stock = work / "stock"
    wheels = gate.stage(work / "wheels", stock)
    subprocess.run(python("verify_templates.py", site), check=True)
    graph = work / "graph"
    subprocess.run([*python("freeze.py", site), graph], check=True)
    extension = work / "extension"
    subprocess.run(
        [
            sys.executable,
            "-IB",
            HERE / "storage/build.py",
            "--imre",
            compiler,
            "--output-dir",
            extension,
            "--cc",
            args.clang,
        ],
        check=True,
    )
    binary, llvm = work / "server-O3", work / "server-O3.ll"
    subprocess.run(
        [
            compiler,
            "build",
            graph / "serve.py",
            "--module-root",
            graph,
            "--runtime",
            runtime,
            "--clang",
            args.clang,
            "--extension-path",
            extension,
            "--no-cache",
            "--opt-level",
            "3",
            "--emit-llvm",
            llvm,
            "-o",
            binary,
        ],
        check=True,
    )
    deps = subprocess.check_output(["readelf", "-d", binary], text=True)
    if "libsqlite3.so" not in deps or "libpython" in deps:
        raise RuntimeError("expected a native executable linked against SQLite")
    lines = llvm.read_text().splitlines()
    dispatches = sum("call " in line and "@imre_dyn_" in line for line in lines)
    sqlite_calls = sum(
        "call " in line and "@imre_extension_call(" in line and "@PyInit__tracker_sqlite" in line
        for line in lines
    )
    if dispatches or sqlite_calls != 1:
        raise RuntimeError(f"unexpected LLVM calls: dynamic={dispatches}, SQLite={sqlite_calls}")
    info = {
        "python": sys.version.split()[0],
        "sqlite_version": sqlite3.sqlite_version,
        "storage_contract_sha256": verify_storage(
            compiler, runtime, args.clang, extension, work / "storage-contract"
        ),
        "dynamic_dispatch_call_sites": dispatches,
        "sqlite_extension_call_sites": sqlite_calls,
        "wheels": wheels,
        "compiler_sha256": prepare.digest(compiler),
        "runtime_sha256": prepare.digest(runtime),
        "native_sha256": prepare.digest(binary),
        "llvm_sha256": prepare.digest(llvm),
        "source_sha256": {
            str(p.relative_to(graph)): prepare.digest(p) for p in sorted(graph.rglob("*.py"))
        },
    }
    env = {
        key: secrets.token_hex(32)
        for key in ("FLASK_SECRET_KEY", "TRACKER_ALICE_TOKEN", "TRACKER_BOB_TOKEN")
    }
    db = work / "seed.sqlite"
    seed(db, env)
    attested_db = work / "attested.sqlite"
    traffic.copy_db(db, attested_db)
    info["native_admission"] = attest(binary, attested_db, env, server, work)
    (work / "build.json").write_text(json.dumps(info, indent=2) + "\n")
    traffic.run(
        {"stock-cpython": python("stock.py", stock), "frozen-imre": [str(binary)]},
        db,
        work / "results.json",
        smoke=args.smoke,
        server_cpu=server,
        client_cpus=clients,
        env=env,
    )
    print(f"Results: {work / 'results.json'}")


if __name__ == "__main__":
    main()
