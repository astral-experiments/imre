"""Full reports, typed JSON failures, and filesystem effects for memory_report."""

import argparse
import collections
import json
import os
import random
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit("validation requires Python assertions; do not use -O")

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "memory_report"
ENTRY = PACKAGE / "__main__.py"
STDLIB = ROOT / "stdlib"
UPSTREAM = PACKAGE / "upstream/memory_report.py"
files = SimpleNamespace(**runpy.run_path(str(HERE / "manifest_validate.py")))
Case = files.Case
case_record = files.case_record
case_from_record = files.case_from_record
materialize = files.materialize
encoded_files = files.encoded_files
digest = files.digest
BOOTSTRAP = (
    "import runpy, sys; sys.path[:0] = sys.argv[1:3]; "
    "sys.argv = ['memory_report', *sys.argv[3:]]; "
    "runpy.run_module('memory_report', run_name='__main__', alter_sys=True)"
)
ORIGINAL_PUBLISH = """
import tempfile
from pathlib import Path
def publish_report(path, rendered):
    with tempfile.TemporaryDirectory(dir=path.parent, prefix='.imre-report-') as name:
        staged = Path(name) / 'report.md'
        staged.write_text(rendered, encoding='utf-8')
        staged.replace(path)
"""
ORIGINAL_COMPARE = ORIGINAL_PUBLISH + """
import runpy, sys
source = runpy.run_path(sys.argv[1])
sys.argv = sys.argv[1:]
args = source['parse_args']()
class AtomicOutput:
    def __init__(self, path):
        self.path = path
    def __str__(self):
        return str(self.path)
    def write_text(self, rendered, encoding='utf-8'):
        assert encoding == 'utf-8'
        publish_report(self.path, rendered)
        return len(rendered)
if args.output is not None:
    args.output = AtomicOutput(args.output)
source['cmd_compare'](args)
"""
ORIGINAL_COMPUTE = """
import json, runpy, sys
from pathlib import Path
source = runpy.run_path(sys.argv[1])
def decode(text):
    data = json.loads(text)
    return source['MemoryReport'](
        total_bytes=data['total_bytes'], struct_metadata_bytes=data['struct_metadata_bytes'],
        struct_fields_bytes=data['struct_fields_bytes'], memo_metadata_bytes=data['memo_metadata_bytes'],
        memo_fields_bytes=data['memo_fields_bytes'], structs=data.get('structs', []), queries=data.get('queries', []),
    )
rounds = int(sys.argv[2])
old_texts = {path.stem: path.read_text(encoding='utf-8') for path in Path(sys.argv[3]).glob('*.json')}
new_texts = {path.stem: path.read_text(encoding='utf-8') for path in Path(sys.argv[4]).glob('*.json')}
checksum, rendered = 0, ''
for _ in range(rounds):
    old = {name: decode(text) for name, text in old_texts.items()}
    new = {name: decode(text) for name, text in new_texts.items()}
    names = sorted(old.keys() & new.keys())
    projects = [source['ProjectComparison'](name=name, old=old[name], new=new[name]) for name in names]
    rendered = source['render_summary'](projects)
    checksum += len(rendered)
print(checksum)
print(rendered)
"""


def python_command(python):
    return [python, "-I", "-B", "-c", BOOTSTRAP, str(PACKAGE_ROOT), str(STDLIB)]


def original_command(python):
    # -I would ignore PYTHONHASHSEED and randomize upstream's equal-change ties.
    return [python, "-B", "-P", "-s", "-c", ORIGINAL_COMPARE, str(UPSTREAM), "compare"]


def compute_python_command(python):
    code = BOOTSTRAP.replace("run_module('memory_report'", "run_module('memory_report.compute'")
    return [python, "-I", "-B", "-c", code, str(PACKAGE_ROOT), str(STDLIB)]


def compute_original_command(python):
    return [python, "-B", "-c", ORIGINAL_COMPUTE, str(UPSTREAM)]


def environment():
    result = {**os.environ, "PYTHONHASHSEED": "0", "PYTHONPATH": ""}
    result.pop("PYTHONHOME", None)
    return result


def report(total=0, structs=None, queries=None):
    return {"total_bytes": total, "struct_metadata_bytes": 0, "struct_fields_bytes": 0,
            "memo_metadata_bytes": 0, "memo_fields_bytes": 0,
            "structs": structs or [], "queries": queries or []}


def encode_report(value):
    return value if isinstance(value, bytes) else (json.dumps(value, ensure_ascii=True) + "\n").encode()


def case(label, old, new, output="report.md"):
    encode = encode_report
    arguments = ("--old", "old.json", "--new", "project.json")
    initial = {"old.json": encode(old), "project.json": encode(new)}
    if output is not None:
        arguments += ("--output", output)
        initial[output] = b"replace the complete report"
    return Case(label, initial, arguments)



def directory_case(label, old, new, output="report.md", extra=None):
    initial = {**{"old/" + name: encode_report(value) for name, value in old.items()},
               **{"new/" + name: encode_report(value) for name, value in new.items()},
               **(extra or {})}
    arguments = ("--old-dir", "old", "--new-dir", "new")
    if output is not None:
        arguments += ("--output", output)
        initial[output] = b"replace the complete report"
    return Case(label, initial, arguments, ("old", "new"))

def integer(text):
    negative = text.startswith("-")
    digits = text[1:] if negative else text
    value = 0
    for start in range(0, len(digits), 9):
        chunk = digits[start:start + 9]
        value = value * 10 ** len(chunk) + int(chunk)
    return -value if negative else value


def format_bytes(value):
    value = float(value)
    for unit in ("B", "kB", "MB"):
        if abs(value) < 1024:
            return f"{value:.2f}{unit}"
        value /= 1024
    return f"{value:.2f}GB"


def format_diff(old, new):
    difference = new - old
    if not difference:
        return "-"
    sign = "+" if difference > 0 else ""
    if not old:
        return f"{sign}{format_bytes(difference)} (new)"
    return f"{sign}{difference / old:.2%} ({format_bytes(abs(difference))})"


def outcome(old, new):
    return "⏫" if new > old else "⬇️" if new < old else "✅"


def expected_projects(projects):
    """Rank plain dictionary/tuple inputs independently of the typed port."""
    projects = sorted(projects, key=lambda project: project[2]["total_bytes"] - project[1]["total_bytes"], reverse=True)
    if not projects:
        return "No memory reports to compare.", []
    if not any(abs(new["total_bytes"] - old["total_bytes"]) > 10_000 for _, old, new in projects):
        return "## Memory usage report\n\nMemory usage unchanged ✅", []
    lines = ["## Memory usage report", "", "### Summary", "",
             "| Project | Old | New | Diff | Outcome |",
             "|---------|-----|-----|------|---------|"]
    for name, old, new in projects:
        old_total, new_total = old["total_bytes"], new["total_bytes"]
        lines.append(f"| {name} | {format_bytes(old_total)} | {format_bytes(new_total)} | {format_diff(old_total, new_total)} | {outcome(old_total, new_total)} |")
    lines.extend(["", "### Significant changes", "", "<details>",
                  "<summary>Click to expand detailed breakdown</summary>", ""])
    groups = []
    for project, old, new in projects:
        old_items = {item["name"]: item.get("metadata_bytes", 0) + item.get("fields_bytes", 0)
                     for item in old.get("structs", []) + old.get("queries", [])}
        new_items = {item["name"]: item.get("metadata_bytes", 0) + item.get("fields_bytes", 0)
                     for item in new.get("structs", []) + new.get("queries", [])}
        names = dict.fromkeys([*old_items, *new_items])
        changes = [(name, old_items.get(name, 0), new_items.get(name, 0)) for name in names
                   if old_items.get(name, 0) != new_items.get(name, 0)]
        changes.sort(key=lambda item: abs(item[2] - item[1]), reverse=True)
        cutoff = abs(changes[min(14, len(changes) - 1)][2] - changes[min(14, len(changes) - 1)][1]) if changes else 0
        rows = [(f"| `{name.replace(' ', '')}` | {format_bytes(before)} | {format_bytes(after)} | {format_diff(before, after)} |{outcome(before, after)} |", abs(after - before))
                for name, before, after in changes if abs(after - before) >= cutoff]
        if rows:
            groups.append(rows)
            lines.extend([f"### {project}", "", "| Name | Old | New | Diff | Outcome |",
                          "|------|-----|-----|------|---------|"])
            lines.extend(row for row, _ in rows[:15])
            if len(changes) > 15:
                lines.append(f"| ... | | | *{len(changes) - 15} more* |")
            lines.append("")
    lines.extend(["</details>", ""])
    return "\n".join(lines), groups


def expected_report(old, new, name):
    rendered, groups = expected_projects([(name, old, new)])
    return rendered, [row for group in groups for row in group]

def options(value):
    arguments = value.arguments[1:] if value.arguments[:1] == ("compare",) else value.arguments
    return dict(zip(arguments[::2], arguments[1::2], strict=True))


def project_name(path):
    # Spell out Python 3.14 Path.stem so the oracle also runs on older hosts.
    name = str(Path(path)).rsplit("/", 1)[-1]
    dot = name.rfind(".")
    return name[:dot] if dot >= 0 and any(char != "." for char in name[:dot]) else name


def oracle(value, error=None):
    expected = {"error": error, "files": dict(value.files),
                "directories": sorted(files.directory_names(value) - {"."})}
    for name, target in value.hardlinks.items():
        expected["files"][name] = expected["files"][target]
    if error:
        return expected
    flags = options(value)
    if flags.get("--old") and flags.get("--new"):
        old = json.loads(value.files[str(Path(flags["--old"]))], parse_int=integer)
        new = json.loads(value.files[str(Path(flags["--new"]))], parse_int=integer)
        projects = [(project_name(flags["--new"]), old, new)]
    else:
        def load(directory):
            prefix = str(Path(directory)).rstrip("/") + "/"
            return {project_name(path): json.loads(data, parse_int=integer)
                    for path, data in value.files.items()
                    if path.startswith(prefix) and "/" not in path[len(prefix):] and path.endswith(".json")}
        old, new = load(flags["--old-dir"]), load(flags["--new-dir"])
        projects = [(name, old[name], new[name]) for name in sorted(old.keys() & new.keys())]
    rendered, groups = expected_projects(projects)
    expected.update(rendered=rendered, rows=[row for group in groups for row in group], row_groups=groups,
                    output=str(Path(flags["--output"])) if "--output" in flags else None)
    return expected


def apply_output(value, expected, rendered):
    result = dict(expected["files"])
    output = expected["output"]
    if output is None:
        return result
    output = os.path.normpath(output)
    data = rendered.encode()
    # Atomic publication replaces the selected entry, leaving other hardlinks
    # (including a report input) attached to the original inode and bytes.
    result[output] = data
    return result


def equivalent_original(rendered, expected):
    """Allow only upstream's set-dependent order/selection among equal changes."""
    def partition(text):
        skeleton, groups = [], []
        in_rows = False
        for line in text.splitlines(keepends=True):
            if line.startswith("| `"):
                if not in_rows:
                    skeleton.append(None)
                    groups.append([])
                groups[-1].append(line.rstrip("\n"))
                in_rows = True
            else:
                skeleton.append(line)
                in_rows = False
        return skeleton, groups

    actual_skeleton, actual_groups = partition(rendered)
    canonical_skeleton, _ = partition(expected["rendered"])
    assert actual_skeleton == canonical_skeleton
    assert len(actual_groups) == len(expected["row_groups"])
    for selected, rows in zip(actual_groups, expected["row_groups"], strict=True):
        available = collections.Counter(row for row, _ in rows)
        assert not (collections.Counter(selected) - available)
        magnitudes = {row: magnitude for row, magnitude in rows}
        assert [magnitudes[row] for row in selected] == [magnitude for _, magnitude in rows[:15]]


def snapshot(directory, symlinks):
    if not symlinks:
        return files.snapshot(directory)
    actual, directories, observed_links = {}, [], {}
    for path in sorted(directory.rglob("*")):
        name = path.relative_to(directory).as_posix()
        if path.is_symlink():
            observed_links[name] = str(path.readlink())
        elif path.is_dir():
            directories.append(name)
        elif path.is_file():
            actual[name] = path.read_bytes()
        else:
            raise AssertionError(f"unexpected filesystem entry: {name}")
    assert observed_links == symlinks
    return actual, directories


def check_result(value, expected, process, directory, engine="port"):
    symlinks = expected.get("symlinks", {})
    actual, directories = snapshot(directory, symlinks)
    stdout, stderr = process.stdout.decode(), process.stderr.decode()
    if expected["error"]:
        assert process.returncode != 0 and not stdout, (value.label, engine, process)
        category = expected["error"]
        allowed = {"OSError": r"(?:OSError|FileNotFoundError|IsADirectoryError|NotADirectoryError|PermissionError|FileExistsError)",
                   "ValueError": r"(?:ValueError|UnicodeDecodeError|JSONDecodeError)"}.get(category, category)
        assert re.search(allowed + r":", stderr), (value.label, engine, stderr)
        expected_files = expected["files"]
    else:
        assert process.returncode == 0, (value.label, engine, process)
        if engine == "original":
            if expected["output"] is None:
                assert not stderr and stdout.endswith("\n")
                rendered = stdout[:-1]
            else:
                prefix = f"Output written to {expected['output']}\n"
                assert not stdout and stderr.startswith(prefix) and stderr.endswith("\n")
                rendered = stderr[len(prefix):-1]
            equivalent_original(rendered, expected)
        elif engine == "original-prepared":
            assert not stderr
            if expected["output"] is None:
                assert stdout.endswith("\n")
                rendered = stdout[:-1]
            else:
                assert not stdout
                rendered = actual[os.path.normpath(expected["output"])].decode()
            equivalent_original(rendered, expected)
        else:
            rendered = expected["rendered"]
            assert not stderr and stdout == (rendered + "\n" if expected["output"] is None else ""), (value.label, engine, stdout, stderr)
        expected_files = apply_output(value, expected, rendered)
    assert actual == expected_files and directories == expected["directories"], (value.label, engine, "file effects")
    return {"returncode": process.returncode, "stdout": stdout, "stderr": stderr,
            "files": {name: {"sha256": digest(data), "bytes": len(data)} for name, data in sorted(actual.items())},
            "directories": directories, "symlinks": symlinks}


def compute_case(value, rounds):
    flags = options(value)
    return Case(value.label + f"-compute-{rounds}", value.files,
                (str(rounds), flags["--old-dir"], flags["--new-dir"]), value.directories, value.hardlinks)


def compute_oracle(value):
    rounds, old, new = value.arguments
    expected = oracle(Case(value.label, value.files, ("--old-dir", old, "--new-dir", new),
                           value.directories, value.hardlinks))
    expected["rounds"] = int(rounds)
    if not int(rounds):
        expected.update(rendered="", rows=[], row_groups=[])
    return expected


def check_compute_result(value, expected, process, directory, engine="port"):
    actual, directories = files.snapshot(directory)
    stdout, stderr = process.stdout.decode(), process.stderr.decode()
    assert process.returncode == 0 and not stderr, (value.label, engine, process)
    checksum, rendered = stdout.split("\n", 1)
    assert rendered.endswith("\n")
    rendered = rendered[:-1]
    if engine == "original":
        equivalent_original(rendered, expected)
    else:
        assert rendered == expected["rendered"], (value.label, engine, rendered)
    assert checksum == str(expected["rounds"] * len(rendered)), (value.label, engine, checksum)
    assert actual == expected["files"] and directories == expected["directories"], (value.label, engine, "file effects")
    return {"returncode": process.returncode, "stdout": stdout, "stderr": stderr,
            "files": {name: {"sha256": digest(data), "bytes": len(data)} for name, data in sorted(actual.items())},
            "directories": directories}


def cases():
    result = []
    def add(value, error=None, original=True):
        result.append((value, error, original and error is None))
    for name in ("inventory", "ruff-scripts"):
        old = (PACKAGE / f"sample/{name}-old.json").read_bytes()
        new = (PACKAGE / f"sample/{name}.json").read_bytes()
        add(case("real-" + name, old, new))
        add(case("real-stdout-" + name, old, new, None))
    for difference in (-10001, -10000, 0, 10000, 10001):
        add(case(f"threshold-{difference}", report(20000), report(20000 + difference)))
    for count in (0, 1, 15, 16, 40):
        old = report(100000, [{"name": f"item {i}", "metadata_bytes": i * 100 + 1} for i in range(count)])
        new = report(200000, [{"name": f"item {i}", "metadata_bytes": i * 100 + (i % 3) * 17 + 1} for i in range(count)])
        add(case(f"ties-and-cutoff-{count}", old, new))
    add(case("added-removed-duplicate-unicode", report(0, [{"name": "α β", "fields_bytes": 1}, {"name": "gone", "metadata_bytes": 7}, {"name": "α β", "metadata_bytes": 8}]),
             report(20000, [{"name": "α β", "fields_bytes": 10}, {"name": "new🙂", "metadata_bytes": 99}])) )
    for index, number in enumerate((2**53 + 1, 2**63 - 1, 2**63, 2**64, 10**100, -(2**63), -(10**100))):
        add(case(f"wide-{index}", report(number, [{"name": "wide", "metadata_bytes": number, "fields_bytes": number}]),
                 report(number + 20001, [{"name": "wide", "metadata_bytes": number, "fields_bytes": number + 12345}])))
    randomizer = random.Random(20261021)
    for index in range(20):
        count = randomizer.randrange(40)
        old = report(randomizer.randrange(-100000, 100001), [{"name": f"item {i % 23}", "metadata_bytes": randomizer.randrange(-100000, 100001)} for i in range(count)])
        new = report(randomizer.randrange(-100000, 100001), [{"name": f"item {i % 29}", "fields_bytes": randomizer.randrange(-100000, 100001)} for i in range(count)])
        add(case(f"random-{index}", old, new))
    minimal = {key: 0 for key in report() if key not in ("structs", "queries")}
    add(case("missing-list-defaults", minimal, {**minimal, "total_bytes": 20000, "structs": [{"name": "default"}]}))
    good = json.dumps(report(20000)).encode()
    for index, malformed in enumerate((b"", b"null", b"[]", b"{}", b"true", b"1", b"{", b"\xff",
                                       good + b" trailing", good.replace(b'20000', b'true', 1), good.replace(b'20000', b'2e4', 1),
                                       good.replace(b'[]', b'null', 1), good.replace(b'[]', b'[{}]', 1),
                                       good.replace(b'20000', b'{"$serde_json::private::Number":"20000"}', 1))):
        add(case(f"invalid-{index}", report(), malformed), "ValueError")
    duplicate = good[:-1] + b',"total_bytes":20000}'
    add(case("duplicate-known", report(), duplicate), "ValueError")
    add(case("duplicate-unknown", report(), good[:-1] + b',"unknown":1,"unknown":2}'))
    add(case("ignored-surrogate", report(), good[:-1] + b',"unknown":{"x":"\\ud800"}}'))
    for field in (b'"name":"\\ud800"', b'"name":"ok","metadata_bytes":null'):
        add(case("invalid-item-" + str(len(result)), report(), good.replace(b'"structs": []', b'"structs": [{' + field + b'}]')), "ValueError")
    for depth in (127, 128):
        text = good[:-1] + b',"unknown":' + b'[' * depth + b'0' + b']' * depth + b'}'
        add(case(f"depth-{depth + 1}", report(), text), "ValueError" if depth == 128 else None)
    huge = b'1' + b'0' * 5000
    suppressed = good.replace(b'20000', huge, 1)
    add(case("unbounded-integer-suppressed", suppressed, suppressed), original=False)
    add(case("float-overflow-before-output", report(), suppressed), "OverflowError")
    normal = case("normal", report(0), report(20000, [{"name": "x", "fields_bytes": 3}]))
    add(Case("overwrite-old", normal.files, ("--old", "old.json", "--new", "project.json", "--output", "old.json")))
    add(Case("hardlink-output", {key: value for key, value in normal.files.items() if key != "report.md"}, normal.arguments,
             hardlinks={"report.md": "project.json"}))
    add(Case("missing-old", {"report.md": b"keep"}, normal.arguments), "OSError")
    add(Case("output-directory", {key: value for key, value in normal.files.items() if key != "report.md"}, normal.arguments, ("report.md",)), "OSError")
    add(Case("output-missing-parent", normal.files, ("--old", "old.json", "--new", "project.json", "--output", "missing/report.md")), "OSError")
    add(Case("unknown-flag", normal.files, (*normal.arguments, "--bad", "x")), "ValueError")
    add(Case("normalized-file-and-output-paths", normal.files,
             ("--old", "./old.json/", "--new", "./project.json//", "--output", "./report.md//")))
    old = {"alpha.json": report(0, [{"name": "same", "fields_bytes": 1}]),
           "beta.json": report(50000, [{"name": "same", "fields_bytes": 50}]),
           "old-only.json": report(9)}
    new = {"alpha.json": report(20000, [{"name": "same", "fields_bytes": 20}]),
           "beta.json": report(0, [{"name": "same", "fields_bytes": 2}]),
           "new-only.json": report(8)}
    add(directory_case("directory-common-projects", old, new))
    add(directory_case("directory-stdout", old, new, None))
    add(directory_case("directory-ignored-nested-and-nonjson", old, new,
                       extra={"old/ignored.txt": b"not JSON", "new/nested/ignored.json": b"not JSON"}))
    add(directory_case("directory-dotfile-unicode-and-multiple-suffix", {".hidden.json": report(), "α β.tar.json": report()},
                       {".hidden.json": report(20001), "α β.tar.json": report(20002)}))
    add(directory_case("directory-stable-project-ties", {"b.json": report(), "a.json": report()},
                       {"b.json": report(20001), "a.json": report(20001)}))
    add(directory_case("directory-ignored-schema-fields", {"a.json": {**report(), "extra": {"nested": [1, "x"]}}},
                       {"a.json": {**report(20001), "extra": True}}))
    add(directory_case("directory-unchanged", old, old))
    add(directory_case("directory-empty", {}, {}), "ValueError")
    add(directory_case("directory-disjoint", {"a.json": report()}, {"b.json": report()}), "ValueError")
    add(directory_case("directory-invalid-unmatched", {**old, "unmatched.json": b"{"}, new), "ValueError")
    add(directory_case("directory-invalid-new", old, {**new, "alpha.json": b"{"}), "ValueError")
    add(directory_case("directory-big-integers", {"wide.json": report(10**100)},
                       {"wide.json": report(10**100 + 20001)}))
    add(directory_case("directory-overflow-before-output", {"wide.json": report()},
                       {"wide.json": suppressed}), "OverflowError")
    normal_directory = directory_case("directory-normal", old, new)
    add(Case("normalized-directory-paths", normal_directory.files,
             ("--old-dir", "./old//", "--new-dir", "./new//", "--output", "./report.md/"), ("old", "new")))
    add(Case("directory-missing", {"report.md": b"keep"}, normal_directory.arguments), "ValueError")
    add(Case("directory-file-as-directory", {"old": b"file", "report.md": b"keep"}, normal_directory.arguments, ("new",)), "ValueError")
    add(Case("directory-matching-subdirectory", normal_directory.files, normal_directory.arguments,
             ("old", "new", "old/not-a-file.json")), "OSError")
    add(Case("directory-output-overwrites-input", normal_directory.files,
             ("--old-dir", "old", "--new-dir", "new", "--output", "old/alpha.json"), ("old", "new")))
    add(Case("directory-output-missing-parent", normal_directory.files,
             ("--old-dir", "old", "--new-dir", "new", "--output", "missing/report.md"), ("old", "new")), "OSError")
    assert len({value.label for value, _, _ in result}) == len(result)
    return result


def validate_commands(commands, timeout=30):
    results = []
    for value, error, original in cases():
        expected = oracle(value, error)
        engines = {}
        for name, command in commands.items():
            if name == "original" and not original:
                continue
            with tempfile.TemporaryDirectory(prefix="memory-validate-") as temporary:
                directory = Path(temporary)
                materialize(value, directory)
                process = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                         cwd=directory, env=environment(), timeout=timeout, check=False)
                engines[name] = check_result(value, expected, process, directory, name)
        results.append({"case": case_record(value), "expected_error": error, "engines": engines})
    return {"cases": len(results), "engine_executions": sum(len(item["engines"]) for item in results),
            "engines": list(commands), "results": results}


def validate_compute_commands(commands, timeout=30):
    results = []
    selected = {"directory-common-projects", "directory-dotfile-unicode-and-multiple-suffix",
                "directory-stable-project-ties", "directory-big-integers", "directory-unchanged",
                "directory-ignored-schema-fields"}
    for ordinary, error, original in cases():
        if ordinary.label not in selected:
            continue
        assert error is None and original
        for rounds in (0, 2):
            value = compute_case(ordinary, rounds)
            expected = compute_oracle(value)
            engines = {}
            for name, command in commands.items():
                with tempfile.TemporaryDirectory(prefix="memory-compute-validate-") as temporary:
                    directory = Path(temporary)
                    materialize(value, directory)
                    process = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                             cwd=directory, env=environment(), timeout=timeout, check=False)
                    engines[name] = check_compute_result(value, expected, process, directory, name)
            results.append({"case": case_record(value), "engines": engines})
    return {"cases": len(results), "engine_executions": sum(len(item["engines"]) for item in results),
            "engines": list(commands), "results": results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--compute-binary", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    args.python = str(Path(shutil.which(args.python) or args.python).resolve(strict=True))
    commands = {"original": original_command(args.python), "port": python_command(args.python)}
    if args.binary:
        commands["imre"] = [str(args.binary.resolve(strict=True))]
    if args.rust:
        commands["rust"] = [str(args.rust.resolve(strict=True))]
    result = validate_commands(commands)
    prepared = SimpleNamespace(**runpy.run_path(str(HERE / "memory_process.py")))
    prepared_commands = {**commands, "original": prepared.original_command(args.python)}
    result["processes"] = prepared.validate_commands(prepared_commands, args.python)
    if args.compute_binary:
        compute_commands = {"original": compute_original_command(args.python), "port": compute_python_command(args.python),
                            "imre": [str(args.compute_binary.resolve(strict=True))]}
        if args.rust:
            compute_commands["rust"] = [str(args.rust.resolve(strict=True)), "compute"]
        result["compute"] = validate_compute_commands(compute_commands)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Passed {result['cases']} memory report cases, {result['engine_executions']} engine executions")
    print(f"Passed {result['processes']['cases']} prepared-project cases, {result['processes']['engine_executions']} engine executions")
    if "compute" in result:
        print(f"Passed {result['compute']['cases']} compute cases, {result['compute']['engine_executions']} engine executions")


if __name__ == "__main__":
    main()
