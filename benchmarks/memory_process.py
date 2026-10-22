"""Prepared-project adapters and independent process/report validation."""

import json
import os
import runpy
import subprocess
import tempfile
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
memory = SimpleNamespace(**runpy.run_path(str(HERE / "memory_validate.py")))

# This adapter deliberately uses the unchanged upstream models and renderer,
# while replacing its network/setup CLI with the prepared-project manifest.
ORIGINAL = """
import json, os, runpy, subprocess, sys
from pathlib import Path
source = runpy.run_path(sys.argv[1])
arguments = sys.argv[2:]
if not arguments or arguments.pop(0) != 'run':
    raise ValueError('expected run mode')
flags = {}
for index in range(0, len(arguments), 2):
    if index + 1 >= len(arguments) or arguments[index] not in ('--projects', '--old-ty', '--new-ty', '--output'):
        raise ValueError('invalid prepared-project arguments')
    flags[arguments[index]] = arguments[index + 1]
if not all(flags.get(key) for key in ('--projects', '--old-ty', '--new-ty')):
    raise ValueError('missing prepared-project arguments')
if not all(flags[key].startswith('/') for key in ('--old-ty', '--new-ty')):
    raise ValueError('ty executable paths must be absolute')
manifest = json.loads(Path(flags['--projects']).read_text(encoding='utf-8'))
if type(manifest) is not dict or type(manifest.get('projects')) is not list or not manifest['projects']:
    raise ValueError('expected nonempty projects list')
names = set()
for project in manifest['projects']:
    if type(project) is not dict or type(project.get('name')) is not str or not project['name'] or project['name'] in names:
        raise ValueError('project names must be nonempty and unique')
    names.add(project['name'])
    if type(project.get('cwd')) is not str or not project['cwd']:
        raise ValueError('project cwd must be nonempty')
    if type(project.get('argv')) is not list or not all(type(item) is str for item in project['argv']) or '{ty}' not in project['argv']:
        raise ValueError('project argv must contain {ty}')
environment = os.environ.copy()
environment['TY_MEMORY_REPORT'] = 'json'
environment['TY_MAX_PARALLELISM'] = '1'
def collect(project, executable):
    command = [executable if argument == '{ty}' else argument for argument in project['argv']]
    process = subprocess.run(command, cwd=project['cwd'], env=environment, capture_output=True, text=True, encoding='utf-8', check=True)
    data = json.loads(process.stdout)
    return source['MemoryReport'](
        total_bytes=data['total_bytes'], struct_metadata_bytes=data['struct_metadata_bytes'],
        struct_fields_bytes=data['struct_fields_bytes'], memo_metadata_bytes=data['memo_metadata_bytes'],
        memo_fields_bytes=data['memo_fields_bytes'], structs=data.get('structs', []), queries=data.get('queries', []),
    )
projects = []
for project in manifest['projects']:
    old = collect(project, flags['--old-ty'])
    new = collect(project, flags['--new-ty'])
    projects.append(source['ProjectComparison'](name=project['name'], old=old, new=new))
projects.sort(key=lambda project: project.name)
rendered = source['render_summary'](projects)
if '--output' in flags:
    Path(flags['--output']).write_text(rendered, encoding='utf-8')
else:
    print(rendered)
"""

PRODUCER = """
import json, os, signal, sys
from pathlib import Path
role = 'old' if Path(sys.executable).name.endswith('-old') else 'new'
config = json.loads(Path('producer.json').read_text())
assert os.environ['TY_MEMORY_REPORT'] == 'json'
assert os.environ['TY_MAX_PARALLELISM'] == '1'
assert os.environ['RELAR_PROCESS_SENTINEL'] == 'inherited'
assert sys.argv[3:] == config.get('extra_args', [])
with Path('events.log').open('a') as events:
    events.write(role + '\\n')
payload = Path(sys.argv[1 if role == 'old' else 2]).read_bytes()
sys.stdout.buffer.write(payload)
sys.stdout.buffer.flush()
sys.stderr.buffer.write(Path('stderr.bin').read_bytes())
sys.stderr.buffer.flush()
if config.get(role + '_signal'):
    os.kill(os.getpid(), signal.SIGTERM)
sys.exit(config.get(role + '_exit', 0))
"""


def original_command(python):
    # Keep PYTHONHASHSEED/PYTHON_JIT effective while excluding cwd and user site.
    # -I would randomize upstream's set-order ties by ignoring PYTHONHASHSEED.
    return [str(python), "-B", "-P", "-s", "-c", ORIGINAL, str(memory.UPSTREAM)]


def environment():
    """Use an explicit environment without recording or passing host secrets."""
    return {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8",
            "PYTHONHASHSEED": "0", "PYTHON_JIT": "0", "PYTHONPATH": "", "RELAR_PROCESS_SENTINEL": "inherited",
            "TY_MEMORY_REPORT": "overridden", "TY_MAX_PARALLELISM": "overridden",
            "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"}


def case(label, projects, old_ty, new_ty, prefix=(), output="report.md", events=False):
    """Keep expected captures separate from the project's actual command/cwd."""
    initial, descriptors = {}, []
    for index, project in enumerate(projects):
        folder = f"project-{index:04}"
        initial[folder + "/old.json"] = memory.encode_report(project["old"])
        initial[folder + "/new.json"] = memory.encode_report(project["new"])
        initial[folder + "/stderr.bin"] = project.get("stderr", b"")
        if events:
            initial[folder + "/producer.json"] = json.dumps(project.get("producer", {})).encode()
            initial[folder + "/events.log"] = b""
        descriptors.append({"name": project["name"], "cwd": project.get("cwd", folder),
                            "argv": project.get("argv", ["{ty}", *prefix, "old.json", "new.json", *project.get("extra_args", [])])})
    initial["projects.json"] = json.dumps({"projects": descriptors}, ensure_ascii=True).encode()
    arguments = ("run", "--projects", "projects.json", "--old-ty", str(old_ty), "--new-ty", str(new_ty))
    if output is not None:
        arguments += ("--output", output)
        initial.setdefault(output, b"keep this report until every project succeeds")
    return memory.Case(label, initial, arguments)


def oracle(value, error=None, events=None):
    expected = {"error": error, "files": dict(value.files),
                "directories": sorted(memory.files.directory_names(value) - {"."})}
    flags = dict(zip(value.arguments[1::2], value.arguments[2::2], strict=True))
    if events is not None:
        for index, sequence in events.items():
            expected["files"][f"project-{index:04}/events.log"] = sequence.encode()
    elif error is None:
        for path in expected["files"]:
            if path.endswith("/events.log"):
                expected["files"][path] = b"old\nnew\n"
    if error:
        return expected
    manifest = json.loads(value.files[flags["--projects"]])
    projects = [(project["name"], json.loads(value.files[f"project-{index:04}/old.json"], parse_int=memory.integer),
                 json.loads(value.files[f"project-{index:04}/new.json"], parse_int=memory.integer))
                for index, project in enumerate(manifest["projects"])]
    rendered, groups = memory.expected_projects(sorted(projects, key=lambda project: project[0]))
    expected.update(rendered=rendered, rows=[row for group in groups for row in group], row_groups=groups,
                    output=str(Path(flags["--output"])) if "--output" in flags else None)
    return expected


def check_result(value, expected, process, directory, engine):
    return memory.check_result(value, expected, process, directory,
                               "original-prepared" if engine == "original" else engine)


def cases(old_ty, new_ty, producer):
    old = memory.report(100_000, [{"name": "Shared Item", "metadata_bytes": 5}, {"name": "removed", "fields_bytes": 11}])
    new = memory.report(140_000, [{"name": "Shared Item", "metadata_bytes": 20}, {"name": "added", "fields_bytes": 8}])
    projects = [{"name": "zulu", "old": old, "new": new}, {"name": "αlpha", "old": new, "new": old}]

    def make(label, selected=None, output="report.md"):
        return case(label, projects if selected is None else selected, old_ty, new_ty, (str(producer),), output, events=True)

    def change(value, *, manifest=None, arguments=None, files=None):
        contents = {**value.files, **(files or {})}
        if manifest is not None:
            contents["projects.json"] = json.dumps(manifest).encode()
        return memory.Case(value.label, contents, value.arguments if arguments is None else arguments,
                           value.directories, value.hardlinks)

    yield make("prepared-two-projects"), None, True, None
    yield make("prepared-stdout", output=None), None, True, None
    yield make("prepared-output-over-input", output="project-0000/old.json"), None, True, None
    literal = ["space and ünicode", "'quoted'", "$(never-run)", "; & | * ?"]
    yield make("prepared-literal-arguments", [{**projects[0], "extra_args": literal, "producer": {"extra_args": literal}}]), None, True, None
    large = {**old, "ignored": "x" * 200_000}
    yield make("prepared-large-both-pipes", [{"name": "large", "old": large, "new": new, "stderr": b"warning\r\n" * 30_000}]), None, True, None
    yield make("prepared-crlf", [{"name": "crlf", "old": memory.encode_report(old).replace(b"\n", b"\r\n"), "new": memory.encode_report(new).replace(b"\n", b"\r")}]), None, True, None
    huge = memory.report(2 ** 80, [{"name": "huge", "metadata_bytes": 2 ** 79}])
    yield make("prepared-big-integers", [{"name": "huge", "old": huge, "new": new}]), None, True, None
    for label, selected, error, observed, original in (
        ("old-exit", [{**projects[0], "producer": {"old_exit": 7}}], "CalledProcessError", {0: "old\n"}, True),
        ("new-exit", [{**projects[0], "producer": {"new_exit": 9}}], "CalledProcessError", {0: "old\nnew\n"}, True),
        ("signal", [{**projects[0], "producer": {"old_signal": True}}], "CalledProcessError", {0: "old\n"}, True),
        ("old-malformed", [{**projects[0], "old": b"{"}], "ValueError", {0: "old\n"}, True),
        ("new-malformed", [{**projects[0], "new": b"{"}], "ValueError", {0: "old\nnew\n"}, True),
        ("late-failure", [projects[0], {**projects[1], "new": b"{"}], "ValueError", {0: "old\nnew\n", 1: "old\nnew\n"}, True),
        ("stdout-utf8", [{**projects[0], "old": b"\xff"}], "UnicodeDecodeError", {0: "old\n"}, True),
        ("stderr-utf8", [{**projects[0], "stderr": b"\xff"}], "UnicodeDecodeError", {0: "old\n"}, True),
        ("decode-before-exit", [{**projects[0], "old": b"\xff", "producer": {"old_exit": 7}}], "UnicodeDecodeError", {0: "old\n"}, True),
        ("typed-output", [{**projects[0], "old": {**old, "total_bytes": True}}], "ValueError", {0: "old\n"}, False),
        ("missing-cwd", [{**projects[0], "cwd": "missing-project"}], "FileNotFoundError", {}, True),
    ):
        yield make("prepared-" + label, selected), error, original, observed
    for label, manifest in (
        ("empty", {"projects": []}),
        ("missing-list", {}),
        ("wrong-list", {"projects": {}}),
        ("duplicate", {"projects": [{"name": "same", "cwd": ".", "argv": ["{ty}"]}] * 2}),
        ("empty-name", {"projects": [{"name": "", "cwd": ".", "argv": ["{ty}"]}]}),
        ("empty-cwd", {"projects": [{"name": "one", "cwd": "", "argv": ["{ty}"]}]}),
        ("missing-placeholder", {"projects": [{"name": "one", "cwd": ".", "argv": ["ty"]}]}),
        ("empty-argv", {"projects": [{"name": "one", "cwd": ".", "argv": []}]}),
        ("wrong-argument", {"projects": [{"name": "one", "cwd": ".", "argv": ["{ty}", 1]}]}),
    ):
        yield change(make("prepared-manifest-" + label), manifest=manifest), "ValueError", True, {}
    yield change(make("prepared-manifest-json"), files={"projects.json": b"{"}), "ValueError", True, {}
    for label, replacement, error in (("relative-executable", "./ty", "ValueError"), ("missing-executable", "/no-such-relar-ty", "FileNotFoundError"),
                                       ("nonexecutable", str(producer), "PermissionError")):
        value = make("prepared-" + label)
        arguments = list(value.arguments)
        arguments[arguments.index("--old-ty") + 1] = replacement
        yield change(value, arguments=tuple(arguments)), error, True, {}
    value = make("prepared-output-missing-parent", output="missing/output.md")
    value = memory.Case(value.label, {name: data for name, data in value.files.items() if name != "missing/output.md"}, value.arguments)
    yield value, "OSError", True, {0: "old\nnew\n", 1: "old\nnew\n"}


def validate_commands(commands, python, timeout=30):
    results = []
    executable = Path(python).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="memory-process-producer-") as temporary:
        root = Path(temporary)
        producer = root / "producer.py"
        producer.write_text(PRODUCER)
        old_ty, new_ty = root / "ty-old", root / "ty-new"
        old_ty.symlink_to(executable)
        new_ty.symlink_to(executable)
        for value, error, original, events in cases(old_ty, new_ty, producer):
            expected = oracle(value, error, events)
            engines = {}
            for name, command in commands.items():
                if name == "original" and not original:
                    continue
                with tempfile.TemporaryDirectory(prefix="memory-process-validate-") as case_directory:
                    directory = Path(case_directory)
                    memory.materialize(value, directory)
                    process = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                             cwd=directory, env=environment(), timeout=timeout, check=False)
                    engines[name] = check_result(value, expected, process, directory, name)
            results.append({"case": memory.case_record(value), "expected_error": error, "engines": engines})
    return {"cases": len(results), "engine_executions": sum(len(item["engines"]) for item in results),
            "engines": list(commands), "environment": environment(), "producer_source": PRODUCER,
            "producer_sha256": memory.digest(PRODUCER.encode()), "python": str(executable),
            "python_sha256": memory.digest(executable.read_bytes()), "results": results}
