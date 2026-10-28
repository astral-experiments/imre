"""Validate the tracker over TCP and time owner-scoped issue workflows on independent SQLite copies."""

from concurrent.futures import ThreadPoolExecutor
from contextlib import closing, contextmanager
import hashlib
from html import unescape
from html.parser import HTMLParser
from http.client import HTTPConnection
from http.cookies import SimpleCookie
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import statistics
import subprocess
import tempfile
import time


def check(condition, description):
    if not condition:
        raise AssertionError(description)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical(data):
    return json.dumps(data, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


class VisibleText(HTMLParser):
    def __init__(self, source):
        super().__init__()
        self.parts = []
        self.feed(source)
        self.close()

    def handle_data(self, text):
        self.parts.append(text)

    def contains(self, text):
        return text in "".join(self.parts)


class Browser:
    """One client maintains its own signed cookie and opens one connection per request."""

    def __init__(self, port, observations=None):
        self.port = port
        self.cookie = ""
        self.csrf = ""
        self.observations = observations
        self.samples = None

    def request(self, label, method, path, status=200, data=None):
        headers = {"Cookie": self.cookie} if self.cookie else {}
        body = None
        if data is not None:
            body = canonical(data)
            headers["Content-Type"] = "application/json"
        start = time.perf_counter_ns()
        with closing(HTTPConnection("127.0.0.1", self.port, timeout=10)) as conn:
            conn.request(method, path, body, headers)
            response = conn.getresponse()
            raw = response.read()
            elapsed = time.perf_counter_ns() - start
        check(
            response.status == status,
            f"{label}: {method} {path}: expected {status}, got {response.status}: {raw[:200]!r}",
        )
        check(int(response.getheader("Content-Length")) == len(raw), f"{label}: incorrect content length")
        cookie = response.getheader("Set-Cookie")
        if cookie:
            value = SimpleCookie(cookie)["session"]
            check(
                value.value.count(".") >= 2 and value["httponly"], f"{label}: expected signed HttpOnly session"
            )
            self.cookie = "session=" + value.value
        private_read = bool(self.cookie) and method == "GET" and status == 200
        if private_read:
            vary = {h.strip().lower() for h in response.getheader("Vary", "").split(",")}
            check("cookie" in vary, f"{label}: missing Vary: Cookie on private read")
        media = response.getheader("Content-Type", "").partition(";")[0]
        value = json.loads(raw) if media == "application/json" else raw.decode("utf-8")
        if self.observations is not None:
            comparable = value
            if path == "/login" and status == 200:
                comparable = {**value, "csrf": "<session nonce>"}
            check(label not in self.observations, f"duplicate scenario step {label}")
            self.observations[label] = {
                "status": status,
                "media": media,
                "set_cookie": bool(cookie),
                "body_sha256": digest(canonical(comparable) if media == "application/json" else raw),
            }
            if private_read:
                self.observations[label]["vary"] = sorted(vary)
        if self.samples is not None:
            self.samples.append((label, elapsed))
        return value

    def login(self, label, username, token):
        payload = self.request(label, "POST", "/login", data={"username": username, "token": token})
        check(payload["csrf"] != self.csrf, f"{label}: successful login reused its CSRF token")
        self.csrf = payload["csrf"]
        check(
            isinstance(self.csrf, str) and bool(self.csrf) and bool(self.cookie),
            f"{label}: missing session credentials",
        )

    def write(self, label, path, data, status=200):
        return self.request(label, "POST", path, status, {"csrf": self.csrf, **data})


@contextmanager
def serve(command, db, env, cpu):
    server_env = {**os.environ, **env, "TRACKER_DB": str(db)}
    for key in ("TRACKER_ALICE_TOKEN", "TRACKER_BOB_TOKEN"):
        server_env.pop(key, None)
    with tempfile.TemporaryDirectory(dir=db.parent) as temp:
        ready = Path(temp) / "ready"
        with (
            (Path(temp) / "log").open("w+") as log,
            subprocess.Popen(
                ["taskset", "-c", str(cpu), *command, ready],
                env=server_env,
                stdout=log,
                stderr=log,
            ) as proc,
        ):
            try:
                deadline = time.monotonic() + 20
                while not (ready.is_file() and (text := ready.read_text()).isdigit()):
                    if proc.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError("server exited or timed out before publishing its port")
                    time.sleep(0.005)
                yield int(text), proc
            except Exception as exc:
                log.seek(0)
                exc.add_note(log.read()[-2000:])
                raise
            finally:
                proc.terminate()
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()


def identity(proc, port):
    directory = Path(f"/proc/{proc.pid}")
    for entry in (directory / "net/tcp").read_text().splitlines()[1:]:
        fields = entry.split()
        if fields[1] == f"0100007F:{port:04X}" and fields[3] == "0A":
            inode = fields[9]
            break
    else:
        raise AssertionError("no loopback listener for the announced port")
    sockets = {os.readlink(fd) for fd in (directory / "fd").iterdir()}
    check(f"socket:[{inode}]" in sockets, "server process does not own its listener")
    exe = (directory / "exe").resolve()
    with exe.open("rb") as stream:
        exe_sha = hashlib.file_digest(stream, "sha256").hexdigest()
    return {
        "pid": proc.pid,
        "executable": exe.name,
        "exe_sha256": exe_sha,
        "listener_inode": inode,
        "libpython_mapped": "/libpython" in (directory / "maps").read_text(),
    }


def copy_db(source, dest):
    # Back up committed state including WAL pages. Every variant starts from exactly this seed.
    with (
        closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as src,
        closing(sqlite3.connect(dest)) as dst,
    ):
        src.backup(dst)


def db_rows(db):
    with closing(sqlite3.connect(db.as_uri() + "?mode=ro", uri=True)) as cx:
        check(cx.execute("PRAGMA integrity_check").fetchone() == ("ok",), "database integrity check failed")
        return cx.execute(
            "SELECT name, title, body, status FROM issues JOIN users ON users.id = issues.owner ORDER BY title"
        ).fetchall()


def scenario(command, db, env, cpu, case_id):
    trace = {}
    title = f"Release <ticket-{case_id}> & 東京 'review'"
    body = f'Resolve the résumé crash <node> "✓" [{case_id}]'
    with serve(command, db, env, cpu) as (port, proc):
        first = identity(proc, port)
        alice = Browser(port, trace)
        bob = Browser(port, trace)
        alice.request("anonymous read", "GET", "/api/issues", 401)
        alice.request(
            "anonymous write", "POST", "/api/issues", 401, {"title": title, "body": body, "csrf": "none"}
        )
        alice.request("invalid credential", "POST", "/login", 401, {"username": "alice", "token": "invalid"})
        alice.login("alice login", "alice", env["TRACKER_ALICE_TOKEN"])
        bob.login("bob login", "bob", env["TRACKER_BOB_TOKEN"])
        old_csrf = alice.csrf
        alice.login("alice fresh login", "alice", env["TRACKER_ALICE_TOKEN"])
        check(
            len({alice.csrf, old_csrf, bob.csrf}) == 3, "different signed sessions received the same CSRF token"
        )
        for label, extra in (
            ("missing csrf", {}),
            ("foreign csrf", {"csrf": bob.csrf}),
            ("old csrf", {"csrf": old_csrf}),
        ):
            alice.request(label, "POST", "/api/issues", 403, {"title": title, "body": body, **extra})
        tampered = Browser(port, trace)
        prefix, sig = alice.cookie.rsplit(".", 1)
        tampered.cookie = prefix + "." + ("A" if sig[0] != "A" else "B") + sig[1:]
        tampered.request("tampered signature", "GET", "/api/issues", 401)
        alice.write("malformed create", "/api/issues", {"body": body}, 400)
        check(
            alice.request("empty owner list", "GET", "/api/issues") == {"issues": []},
            "unauthorized write changed state",
        )
        ident = alice.write("create issue", "/api/issues", {"title": title, "body": body}, 201)["id"]
        check(type(ident) is int and ident > 0, "create did not return a positive integer ID")
        detail = f"/issues/{ident}"
        api = f"/api/issues/{ident}"
        issue = {"id": ident, "title": title, "body": body, "status": "open"}
        check(
            alice.request("open JSON", "GET", "/api/issues?status=open") == {"issues": [issue]},
            "incorrect created issue",
        )
        check(
            VisibleText(alice.request("open list HTML", "GET", "/issues?status=open")).contains(title),
            "list lost or failed to escape the title",
        )
        html = VisibleText(alice.request("detail HTML", "GET", detail))
        check(html.contains(title) and html.contains(body), "detail lost or failed to escape user text")
        check(
            bob.request("private owner JSON", "GET", "/api/issues") == {"issues": []},
            "another user can list the issue",
        )
        bob.request("private detail", "GET", detail, 404)
        bob.write("private update", api, {"status": "closed"}, 404)
        alice.write("invalid update", api, {"status": "invalid"}, 400)
        alice.request("update without csrf", "POST", api, 403, {"status": "closed"})
        check(
            alice.write("close issue", api, {"status": "closed"}) == {"id": ident},
            "update returned a different ID",
        )
        check(
            alice.request("empty open filter", "GET", "/api/issues?status=open") == {"issues": []},
            "closed issue stayed open",
        )
        check(
            not VisibleText(alice.request("empty open HTML", "GET", "/issues?status=open")).contains(title),
            "closed issue visible in open HTML",
        )
    check(db_rows(db) == [("alice", title, body, "closed")], "SQLite state after server exit differs")
    with serve(command, db, env, cpu) as (port, proc):
        second = identity(proc, port)
        check(first["pid"] != second["pid"], "restart reused first process")
        alice.port = port
        check(
            alice.request("signed session after restart", "GET", "/api/issues?status=closed")
            == {"issues": [{**issue, "status": "closed"}]},
            "restarted server did not recover session and data",
        )
        fresh = Browser(port, trace)
        fresh.login("fresh login after restart", "alice", env["TRACKER_ALICE_TOKEN"])
        check(fresh.csrf not in (old_csrf, alice.csrf, bob.csrf), "CSRF token repeated after restart")
        html = VisibleText(fresh.request("persistent HTML after restart", "GET", detail))
        check(html.contains(title) and html.contains(body), "restarted server lost issue details")
        check(
            fresh.write("reopen after restart", api, {"status": "open"}) == {"id": ident},
            "restart lost update binding",
        )
    check(db_rows(db) == [("alice", title, body, "open")], "post-restart write did not reach SQLite")
    return {
        "http": trace,
        "initial_process": first,
        "restarted_process": second,
        "issue_id": ident,
        "persisted_rows": 1,
        "final_db_content_sha256": digest(canonical(db_rows(db))),
    }


def issue_text(lane, seq):
    title = f"Queue <{lane}:{seq}> & Zürich"
    body = f'Report {lane}/{seq}: 東京, "stale" <state>'
    return title, body


def cycle(client, lane, seq):
    title, body = issue_text(lane, seq)
    ident = client.write("create", "/api/issues", {"title": title, "body": body}, 201)["id"]
    check(type(ident) is int and ident > 0, "invalid created ID")
    detail = VisibleText(client.request("detail", "GET", f"/issues/{ident}"))
    check(detail.contains(title) and detail.contains(body), "rendered detail mismatch")
    check(
        client.write("update", f"/api/issues/{ident}", {"status": "closed"}) == {"id": ident},
        "issue update mismatch",
    )
    values = client.request("filter_json", "GET", "/api/issues?status=closed")["issues"]
    ids = [v["id"] for v in values]
    expected = {"id": ident, "title": title, "body": body, "status": "closed"}
    verify_page(ids, ident, expected in values)
    links = html_issues(client.request("filter_html", "GET", "/issues?status=closed"))
    verify_page([i for i, _ in links], ident, (ident, title) in links)


def html_issues(markup):
    links = re.findall(r'<a href="/issues/([0-9]+)">([^<]*)</a>', markup)
    check(len(links) == markup.count("<li>"), "issue list contains an unescaped or malformed link")
    return [(int(i), unescape(text)) for i, text in links]


def verify_page(ids, ident, found):
    check(0 < len(ids) <= 50 and ids == sorted(set(ids), reverse=True), "page order or size mismatch")
    # The other client for this owner may have created >50 newer issues before this GET ran.
    check(found or (len(ids) == 50 and ids[-1] > ident), "recent issue missing from its owner's page")


def verify_final_pages(db, browsers):
    counts = {}
    with closing(sqlite3.connect(db.as_uri() + "?mode=ro", uri=True)) as cx:
        cx.row_factory = sqlite3.Row
        for user, client in zip(("alice", "bob"), browsers):
            rows = cx.execute(
                "SELECT issues.id, title, body, status FROM issues JOIN users ON users.id = issues.owner "
                "WHERE users.name = ? AND status = 'closed' ORDER BY issues.id DESC LIMIT 50",
                (user,),
            )
            expected = [dict(row) for row in rows]
            # These requests are outside the measured interval.
            audit = Browser(client.port)
            audit.cookie = client.cookie
            check(
                audit.request("final JSON", "GET", "/api/issues?status=closed") == {"issues": expected},
                "final JSON differs from this owner's newest SQLite issues",
            )
            links = html_issues(audit.request("final HTML", "GET", "/issues?status=closed"))
            check(
                links == [(row["id"], row["title"]) for row in expected],
                "final HTML differs from this owner's newest SQLite issues",
            )
            counts[user] = len(links)
    return counts


def process_cpu(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rpartition(") ")[2].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def latency(samples):
    times = sorted(samples)
    return {
        "count": len(times),
        "p50_ms": statistics.median(times) / 1e6,
        "p95_ms": times[math.ceil(0.95 * len(times)) - 1] / 1e6,
    }


def trial(command, db, env, cpu, clients, cycles):
    with serve(command, db, env, cpu) as (port, proc), ThreadPoolExecutor(clients) as pool:
        server = identity(proc, port)
        browsers = []
        for lane in range(clients):
            client = Browser(port)
            who = ("alice", "bob")[lane % 2]
            client.login("login", who, env[f"TRACKER_{who.upper()}_TOKEN"])
            cycle(client, lane, -1)
            client.samples = []
            browsers.append(client)

        def traffic(lane):
            for seq in range(cycles):
                cycle(browsers[lane], lane, seq)

        cpu0 = process_cpu(proc.pid)
        client0 = time.process_time()
        start = time.perf_counter()
        list(pool.map(traffic, range(clients)))
        elapsed = time.perf_counter() - start
        client_cpu = time.process_time() - client0
        cpu_seconds = process_cpu(proc.pid) - cpu0
        final_page_rows = verify_final_pages(db, browsers)

    samples = [item for client in browsers for item in client.samples]
    ops = {
        label: latency([t for op, t in samples if label == op]) for label in sorted({op for op, t in samples})
    }
    check(
        len(ops) == 5 and all(r["count"] == clients * cycles for r in ops.values()),
        "incomplete measured traffic",
    )
    rows = db_rows(db)
    expected = [
        (("alice", "bob")[lane % 2], *issue_text(lane, seq), "closed")
        for lane in range(clients)
        for seq in range(-1, cycles)
    ]
    check(set(rows) == set(expected) and len(rows) == len(expected), "final SQLite state mismatch")
    return {
        "process": server,
        "requests": len(samples),
        "elapsed_s": elapsed,
        "requests_per_s": len(samples) / elapsed,
        "server_cpu_s": cpu_seconds,
        "client_cpu_s": client_cpu,
        "latency": latency([t for _, t in samples]),
        "operations": ops,
        "final_rows": len(rows),
        "final_page_rows": final_page_rows,
        "final_db_content_sha256": digest(canonical(rows)),
    }


def run(commands, seed_db, out, *, smoke=False, server_cpu=18, client_cpus=(20, 21), env=None):
    """Run admission and paired timing. Commands accept a final ready-file argument."""
    seed_db = Path(seed_db).resolve(strict=True)
    out = Path(out).resolve()
    check(not out.exists(), f"output already exists: {out}")
    check(len(commands) >= 2, "expected at least two variants")
    check(db_rows(seed_db) == [], "expected an empty issue table in the seeded database")
    variables = {**os.environ, **(env or {})}
    for key in ("TRACKER_ALICE_TOKEN", "TRACKER_BOB_TOKEN", "FLASK_SECRET_KEY"):
        check(variables.get(key), f"missing {key}")
    prior_affinity = os.sched_getaffinity(0)
    cpus = set(client_cpus)
    check(
        cpus and server_cpu not in cpus and cpus | {server_cpu} <= prior_affinity,
        "provide distinct available server/client CPUs",
    )
    rounds, clients, cycles = (1, 2, 1) if smoke else (5, 4, 100)
    report = {
        "version": 1,
        "server_cpu": server_cpu,
        "client_cpus": sorted(cpus),
        "concurrency": clients,
        "rounds": rounds,
        "cycles_per_client": cycles,
        "warmup_cycles_per_client": 1,
        "page_limit": 50,
        "body_hashing": "Canonical JSON with login nonces normalized; original bytes for HTML.",
        "seed_db_sha256": digest(seed_db.read_bytes()),
        "admission": {},
        "trials": [],
    }
    out.parent.mkdir(parents=True, exist_ok=True)

    def save():
        tmp = out.with_suffix(".partial")
        tmp.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        tmp.replace(out)

    os.sched_setaffinity(0, cpus)
    try:
        with tempfile.TemporaryDirectory(dir=out.parent) as temp:
            root = Path(temp)
            reference = None
            for i, (name, cmd) in enumerate(commands.items()):
                db = root / f"gate-{i}.sqlite"
                copy_db(seed_db, db)
                row = scenario(cmd, db, variables, server_cpu, report["seed_db_sha256"][:12])
                report["admission"][name] = row
                save()
                if reference is not None:
                    check(reference["http"].keys() == row["http"].keys(), "admission steps differ")
                    for step, observed in row["http"].items():
                        check(
                            reference["http"].get(step) == observed,
                            f"HTTP parity failed at {step}: {observed} vs {reference['http'].get(step)}",
                        )
                    check(
                        row["final_db_content_sha256"] == reference["final_db_content_sha256"],
                        "post-restart DB parity failed",
                    )
                reference = row
            final_digest = None
            names = list(commands)
            for r in range(rounds):
                for name in names if r % 2 == 0 else reversed(names):
                    db = root / f"timed-{len(report['trials'])}.sqlite"
                    copy_db(seed_db, db)
                    row = trial(commands[name], db, variables, server_cpu, clients, cycles)
                    check(
                        row["process"]["exe_sha256"]
                        == report["admission"][name]["initial_process"]["exe_sha256"],
                        "timed server executable differs from the one admitted",
                    )
                    if final_digest is not None:
                        check(
                            row["final_db_content_sha256"] == final_digest,
                            "final issue contents differ between trials",
                        )
                    final_digest = row["final_db_content_sha256"]
                    row.update(server=name, round=r + 1)
                    report["trials"].append(row)
                    save()
    finally:
        os.sched_setaffinity(0, prior_affinity)
    return report
