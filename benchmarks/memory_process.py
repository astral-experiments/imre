"""Prepared-project adapters and independent process/report validation."""

import json
import os
import re
import resource
import runpy
import shutil
import signal
import subprocess
import tempfile
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
memory = SimpleNamespace(**runpy.run_path(str(HERE / "memory_validate.py")))

# This adapter deliberately uses the unchanged upstream models and renderer,
# while replacing its network/setup CLI with the prepared-project manifest.
ORIGINAL = memory.ORIGINAL_PUBLISH + """
import json, os, runpy, subprocess, sys
from pathlib import Path
source = runpy.run_path(sys.argv[1])
arguments = sys.argv[2:]
if not arguments or arguments.pop(0) != 'run':
    raise ValueError('expected run mode')
flags = {}
for index in range(0, len(arguments), 2):
    if index + 1 >= len(arguments) or arguments[index] not in ('--projects', '--old-ty', '--new-ty', '--capture-dir', '--output'):
        raise ValueError('invalid prepared-project arguments')
    flags[arguments[index]] = arguments[index + 1]
if not all(flags.get(key) for key in ('--projects', '--old-ty', '--new-ty')):
    raise ValueError('missing prepared-project arguments')
if '--capture-dir' in flags and '--output' in flags:
    if Path(flags['--output']).name in {'projects.json', 'old.stdout.txt', 'old.stderr.txt', 'new.stdout.txt', 'new.stderr.txt'}:
        raise ValueError('output basename is reserved for captured transcripts')
if not all(flags[key].startswith('/') for key in ('--old-ty', '--new-ty')):
    raise ValueError('ty executable paths must be absolute')
manifest_text = Path(flags['--projects']).read_text(encoding='utf-8')
manifest = json.loads(manifest_text)
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
def collect(project, executable, capture, workspace, role):
    command = [executable if argument == '{ty}' else argument for argument in project['argv']]
    process = subprocess.run(command, cwd=project['cwd'], env=environment, capture_output=True, text=True, encoding='utf-8', check=capture is None)
    if capture is not None:
        staged_stdout = workspace / 'stdout.txt'
        staged_stderr = workspace / 'stderr.txt'
        staged_stdout.write_text(process.stdout, encoding='utf-8')
        staged_stderr.write_text(process.stderr, encoding='utf-8')
        staged_stdout.replace(capture / (role + '.stdout.txt'))
        staged_stderr.replace(capture / (role + '.stderr.txt'))
        process.check_returncode()
    data = json.loads(process.stdout)
    return source['MemoryReport'](
        total_bytes=data['total_bytes'], struct_metadata_bytes=data['struct_metadata_bytes'],
        struct_fields_bytes=data['struct_fields_bytes'], memo_metadata_bytes=data['memo_metadata_bytes'],
        memo_fields_bytes=data['memo_fields_bytes'], structs=data.get('structs', []), queries=data.get('queries', []),
    )
def collect_projects(capture_directory=None, workspace=None):
    projects = []
    for index, project in enumerate(manifest['projects']):
        capture = None
        if capture_directory is not None:
            capture = capture_directory / ('project-' + str(index))
            capture.mkdir()
        old = collect(project, flags['--old-ty'], capture, workspace, 'old')
        new = collect(project, flags['--new-ty'], capture, workspace, 'new')
        projects.append(source['ProjectComparison'](name=project['name'], old=old, new=new))
    return sorted(projects, key=lambda project: project.name)
if '--capture-dir' in flags:
    capture_directory = Path(flags['--capture-dir'])
    capture_directory.mkdir()
    with tempfile.TemporaryDirectory(dir=capture_directory, prefix='.relar-capture-') as name:
        workspace = Path(name)
        staged_manifest = workspace / 'projects.json'
        staged_manifest.write_text(manifest_text, encoding='utf-8')
        staged_manifest.replace(capture_directory / 'projects.json')
        projects = collect_projects(capture_directory, workspace)
else:
    projects = collect_projects()
rendered = source['render_summary'](projects)
if '--output' in flags:
    publish_report(Path(flags['--output']), rendered)
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
if role == 'old' and config.get('workspace_symlink'):
    workspace, = Path('../captures').glob('.relar-capture-*')
    workspace.rmdir()
    workspace.symlink_to('../workspace-target', target_is_directory=True)
    Path('workspace-name.txt').write_text(workspace.name, encoding='utf-8')
if role == 'old' and config.get('block_stderr_replace'):
    Path('../captures/project-0/old.stderr.txt').mkdir()
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
            "PYTHONHASHSEED": "0", "PYTHON_JIT": "0", "PYTHONPATH": "", "PYTHONDONTWRITEBYTECODE": "1",
            "RELAR_PROCESS_SENTINEL": "inherited",
            "TY_MEMORY_REPORT": "overridden", "TY_MAX_PARALLELISM": "overridden",
            "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"}


def case(label, projects, old_ty, new_ty, prefix=(), output="report.md", events=False, capture_dir=None):
    """Keep expected captures separate from the project's actual command/cwd."""
    initial, descriptors = {}, []
    for index, project in enumerate(projects):
        folder = f"project-{index:04}"
        initial[folder + "/old.json"] = memory.encode_report(project.get("old_stdout", project["old"]))
        initial[folder + "/new.json"] = memory.encode_report(project.get("new_stdout", project["new"]))
        initial[folder + "/stderr.bin"] = project.get("stderr", b"")
        for role in ("old", "new"):
            if role + "_stderr" in project:
                initial[folder + "/" + role + ".stderr.bin"] = project[role + "_stderr"]
        if events:
            initial[folder + "/producer.json"] = json.dumps(project.get("producer", {})).encode()
            initial[folder + "/events.log"] = b""
        descriptors.append({"name": project["name"], "cwd": project.get("cwd", folder),
                            "argv": project.get("argv", ["{ty}", *prefix, "old.json", "new.json", *project.get("extra_args", [])])})
    initial["projects.json"] = json.dumps({"projects": descriptors}, ensure_ascii=True).encode()
    arguments = ("run", "--projects", "projects.json", "--old-ty", str(old_ty), "--new-ty", str(new_ty))
    if capture_dir is not None:
        arguments += ("--capture-dir", capture_dir)
    if output is not None:
        arguments += ("--output", output)
        initial.setdefault(output, b"keep this report until every project succeeds")
    return memory.Case(label, initial, arguments)


def decoded_text(data):
    """Predict strict UTF-8 text-mode capture and universal-newline conversion."""
    return data.decode("utf-8").replace("\r\n", "\n").replace("\r", "\n").encode("utf-8")


def oracle(value, error=None, events=None, capture_state=None):
    expected = {"error": error, "files": dict(value.files),
                "directories": sorted(memory.files.directory_names(value) - {"."})}
    flags = dict(zip(value.arguments[1::2], value.arguments[2::2], strict=True))
    for name, target in value.hardlinks.items():
        expected["files"][name] = expected["files"][target]
    if events is not None:
        for index, sequence in events.items():
            index = int(index)
            expected["files"][f"project-{index:04}/events.log"] = sequence.encode()
            config = json.loads(value.files.get(f"project-{index:04}/producer.json", b"{}"))
            if "old\n" in sequence and config.get("block_stderr_replace"):
                expected["directories"].append("captures/project-0/old.stderr.txt")
    elif error is None:
        for path in expected["files"]:
            if path.endswith("/events.log"):
                expected["files"][path] = b"old\nnew\n"
    if "--capture-dir" in flags and (error is None or capture_state is not None):
        manifest = json.loads(value.files[flags["--projects"]])
        if capture_state is None:
            capture_state = {str(index): ["old", "new"] for index in range(len(manifest["projects"]))}
        directory = os.path.normpath(str(Path(flags["--capture-dir"])))
        expected["files"][directory + "/projects.json"] = decoded_text(value.files[flags["--projects"]])
        directories = {*expected["directories"], directory}
        for index, roles in capture_state.items():
            folder = directory + "/project-" + str(index)
            directories.add(folder)
            source = f"project-{int(index):04}"
            for selected in roles:
                role, *specific = selected.split(".")
                streams = specific or ("stdout", "stderr")
                assert role in ("old", "new") and all(stream in ("stdout", "stderr") for stream in streams)
                for stream in streams:
                    data = (value.files[source + "/" + role + ".json"] if stream == "stdout" else
                            value.files.get(source + "/" + role + ".stderr.bin", value.files[source + "/stderr.bin"]))
                    expected["files"][folder + "/" + role + "." + stream + ".txt"] = decoded_text(data)
        expected["directories"] = sorted(directories)
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


def reset_captures(value, directory):
    """Restore a benchmark's fresh capture destination before, not during, timing."""
    flags = dict(zip(value.arguments[1::2], value.arguments[2::2], strict=True))
    if "--capture-dir" not in flags:
        return
    capture = Path(flags["--capture-dir"])
    assert len(capture.parts) == 1 and not capture.is_absolute() and capture.name not in ("", ".", "..")
    assert all(capture != Path(name) and capture not in Path(name).parents
               for name in (*value.files, *value.directories, *value.hardlinks))
    target = directory / capture
    if target.exists():
        assert target.is_dir() and not target.is_symlink()
        shutil.rmtree(target)


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


def capture_cases(old_ty, new_ty, producer):
    """Check exact retained transcripts and cleanup on every failure boundary."""
    completed = ["old", "new"]
    failure_states = {
        "old-exit": {"0": ["old"]}, "new-exit": {"0": completed},
        "signal": {"0": ["old"]}, "old-malformed": {"0": ["old"]},
        "new-malformed": {"0": completed}, "late-failure": {"0": completed, "1": completed},
        "stdout-utf8": {"0": []}, "stderr-utf8": {"0": []}, "decode-before-exit": {"0": []},
        "typed-output": {"0": ["old"]}, "missing-cwd": {"0": []},
        "missing-executable": {"0": []}, "nonexecutable": {"0": []},
        "output-missing-parent": {"0": completed, "1": completed},
    }
    for value, error, original, events in cases(old_ty, new_ty, producer):
        suffix = value.label.removeprefix("prepared-")
        state = None
        if error is not None and not suffix.startswith("manifest-") and suffix != "relative-executable":
            state = failure_states[suffix]
        value = replace(value, label="capture-" + suffix,
                        arguments=(*value.arguments, "--capture-dir", "captures"))
        yield value, error, original, events, state

    empty = memory.report()
    selected = [{"name": "one", "old": empty, "new": empty, "stderr": b"warning\r\nsecond\rlast"}]

    def make(label, projects=None, output="report.md", capture_dir="captures"):
        return case("capture-" + label, selected if projects is None else projects, old_ty, new_ty,
                    (str(producer),), output, events=True, capture_dir=capture_dir)

    names = ["../outside", "/absolute", "a/b", "..", "project-0", "old.stdout.txt", "two\nlines", "unicodé"]
    yield make("arbitrary-names", [{**selected[0], "name": name} for name in names]), None, True, None, None
    value = make("manifest-newlines")
    contents = {**value.files, "projects.json": value.files["projects.json"] + b"\r\n\r\n"}
    yield replace(value, files=contents), None, True, None, None
    value = make("nested-directory", capture_dir="retained results/captures/")
    yield replace(value, directories=("retained results",)), None, True, None, None
    value = make("parent-alias", capture_dir="existing/../captures")
    yield replace(value, directories=("existing",)), None, True, None, None
    for name in ("projects.json", "old.stdout.txt", "old.stderr.txt", "new.stdout.txt", "new.stderr.txt"):
        yield make("reserved-output-" + name, output="outside/" + name), "ValueError", True, {}, None
    for output in ("captures/report.md", "captures/project-0/../report.md"):
        value = make("output-inside-" + ("alias" if ".." in output else "capture"), output=output)
        yield replace(value, files={name: data for name, data in value.files.items() if name != output}), None, True, None, None
    value = make("existing-directory")
    yield replace(value, files={**value.files, "captures/keep.txt": b"keep existing transcript"}), "OSError", True, {}, None
    value = make("existing-empty-directory")
    yield replace(value, directories=("captures",)), "OSError", True, {}, None
    value = make("existing-file")
    yield replace(value, files={**value.files, "captures": b"keep existing file"}), "OSError", True, {}, None
    yield make("missing-parent", capture_dir="missing/captures"), "OSError", True, {}, None
    yield make("empty-path", capture_dir=""), "OSError", True, {}, None
    value = make("report-is-directory")
    contents = {name: data for name, data in value.files.items() if name != "report.md"}
    contents["report.md/keep.txt"] = b"keep directory contents"
    yield replace(value, files=contents), "OSError", True, {0: "old\nnew\n"}, {"0": completed}
    value = make("report-hardlink")
    yield replace(value, files={name: data for name, data in value.files.items() if name != "report.md"},
                  hardlinks={"report.md": "project-0000/old.json"}), None, True, None, None
    value = make("workspace-symlink", [{**selected[0], "producer": {"workspace_symlink": True}}])
    yield replace(value, files={**value.files, "project-0000/workspace-name.txt": b"",
                                "workspace-target/keep.txt": b"keep symlink target"}), \
        "OSError", True, {0: "old\nnew\n"}, {"0": completed}
    yield make("second-replace-failure", [{**selected[0], "producer": {"block_stderr_replace": True}}]), \
        "OSError", True, {0: "old\n"}, {"0": ["old.stdout"]}
    for field in ("argv", "cwd"):
        value = make("later-nul-" + field, [selected[0], {**selected[0], "name": "second"}])
        manifest = json.loads(value.files["projects.json"])
        if field == "argv":
            manifest["projects"][1]["argv"].append("embedded\0nul")
        else:
            manifest["projects"][1]["cwd"] += "\0"
        yield replace(value, files={**value.files, "projects.json": json.dumps(manifest).encode()}), \
            "ValueError", True, {0: "old\nnew\n"}, {"0": completed, "1": []}


def fault_cases(old_ty, new_ty, producer):
    """Force staging failures with bounded OS controls, without special app hooks."""
    before = memory.report(100_000, [{"name": "long-name-" + "x" * 80 + str(index), "fields_bytes": 1000}
                                     for index in range(20)])
    after = memory.report(200_000, [{"name": item["name"], "fields_bytes": 2000} for item in before["structs"]])
    projects = [{"name": "one", "old": before, "new": after}]
    value = case("atomic-report-write-limit", projects, old_ty, new_ty, (str(producer),), events=True)
    assert len(oracle(value)["rendered"].encode()) > 512
    yield value, "OSError", True, {0: "old\nnew\n"}, None, {"file_size_limit": 512}
    yield replace(value, label="atomic-report-restrictive-umask"), "OSError", True, {0: "old\nnew\n"}, None, {"umask": 0o700}
    for stream in ("stdout", "stderr"):
        project = projects[0] if stream == "stdout" else {"name": "one", "old": memory.report(), "new": memory.report(), "stderr": b"x" * 2048}
        value = case("capture-" + stream + "-write-limit", [project], old_ty, new_ty, (str(producer),),
                     events=True, capture_dir="captures")
        assert len(decoded_text(value.files["projects.json"])) < 512
        yield value, "OSError", True, {0: "old\n"}, {"0": []}, {"file_size_limit": 512}


def validation_cases(old_ty, new_ty, producer):
    for value, error, original, events in cases(old_ty, new_ty, producer):
        yield value, error, original, events, None, {}
    for value, error, original, events, state in capture_cases(old_ty, new_ty, producer):
        yield value, error, original, events, state, {}
    yield from fault_cases(old_ty, new_ty, producer)


def child_controls(controls):
    if "file_size_limit" in controls:
        limit = controls["file_size_limit"]
        resource.setrlimit(resource.RLIMIT_FSIZE, (limit, limit))
        signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
    if "umask" in controls:
        os.umask(controls["umask"])


def apply_effect_witnesses(value, expected, witnesses):
    """Validate retained cleanup witnesses and add their exact expected effects."""
    if value.label != "capture-workspace-symlink":
        assert witnesses == {}
        return
    assert set(witnesses) == {"failed_cleanup_workspace"}
    workspace = witnesses["failed_cleanup_workspace"]
    assert set(workspace) == {"name", "symlink_target"}
    name = workspace["name"]
    assert re.fullmatch(r"\.relar-capture-[A-Za-z0-9_]+", name)
    assert workspace["symlink_target"] == "../workspace-target"
    expected["files"]["project-0000/workspace-name.txt"] = name.encode()
    expected["symlinks"] = {"captures/" + name: workspace["symlink_target"]}


def effect_witnesses(value, expected, directory):
    """Bind the unpredictable private basename left by a deliberate cleanup error."""
    if value.label != "capture-workspace-symlink":
        return {}
    filename = "project-0000/workspace-name.txt"
    name = (directory / filename).read_text(encoding="utf-8")
    assert re.fullmatch(r"\.relar-capture-[A-Za-z0-9_]+", name)
    workspace = directory / "captures" / name
    assert workspace.is_symlink() and workspace.readlink() == Path("../workspace-target")
    witnesses = {"failed_cleanup_workspace": {"name": name, "symlink_target": "../workspace-target"}}
    apply_effect_witnesses(value, expected, witnesses)
    return witnesses


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
        for value, error, original, events, state, controls in validation_cases(old_ty, new_ty, producer):
            engines = {}
            for name, command in commands.items():
                if name == "original" and not original:
                    continue
                with tempfile.TemporaryDirectory(prefix="memory-process-validate-") as case_directory:
                    directory = Path(case_directory)
                    memory.materialize(value, directory)
                    process = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                             cwd=directory, env=environment(), timeout=timeout, check=False,
                                             preexec_fn=(lambda: child_controls(controls)) if controls else None)
                    expected = oracle(value, error, events, state)
                    witnesses = effect_witnesses(value, expected, directory)
                    engines[name] = {**check_result(value, expected, process, directory, name), "effect_witnesses": witnesses}
            results.append({"case": memory.case_record(value), "expected_error": error,
                            "events": events, "capture_state": state, "child_controls": controls, "engines": engines})
    return {"cases": len(results), "engine_executions": sum(len(item["engines"]) for item in results),
            "engines": list(commands), "environment": environment(), "producer_source": PRODUCER,
            "producer_sha256": memory.digest(PRODUCER.encode()), "python": str(executable),
            "python_sha256": memory.digest(executable.read_bytes()), "results": results}
