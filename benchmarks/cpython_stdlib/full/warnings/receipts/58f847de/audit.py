"""Reconstruct the four environment-option parents and their child compiler invocations."""
import ast
import hashlib
import json

TAG = "public-58f847de"
HEAD = "58f847de7c7c1630fd19446a538058b99673a571"
SUITE = "3b00dac0c24bbaf28b3f442402c95d7850f50b567f94d8378d950513150e0943"
INVENTORY = "e052c72bf31c8b98636aae6da3cd9d8266ab4e991f5410753a05e80760ef48ff"
DEFAULT = "import warnings; [print(f) for f in warnings._get_filters()]"
BLOCK = "import sys; sys.modules.pop('warnings', None); sys.modules['_warnings'] = None; "
CONFLICT = (
    "import sys, warnings; sys.stdout.write(str(sys.warnoptions)); "
    "warnings.warn('Message', DeprecationWarning)"
)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(path):
    return json.loads(path.read_bytes())


def argv(path):
    data = path.read_bytes()
    assert data.endswith(b"\0")
    return [argument.decode() for argument in data[:-1].split(b"\0")]


def original_callbacks(folder, process, names):
    """Require the complete, ordered suite and return each original's four callback lines."""
    for stream in ("stdout", "stderr"):
        assert digest(folder / stream) == process[stream + "_sha256"]
    events = [s.split("\t", 2) for s in (folder / "stdout").read_text().splitlines()]

    def values(label):
        return [event[1:] for event in events if event[0] == label]

    assert values("EXECUTABLE") == [[process["argv"][0]]]
    assert values("PROVIDER_IDENTICAL") == [["C/PurePython", "False"]]
    providers = {name: ast.literal_eval(value) for name, value in values("PROVIDER")}
    expected = {
        "C": ("warnings", "builtin_function_or_method", "_warnings"),
        "PurePython": ("_py_warnings", "function", "_py_warnings"),
    }
    for label, expected_shape in expected.items():
        obj = providers[label]
        assert (obj[0], obj[2], obj[3]) == expected_shape and not obj[4]
    assert values("COUNT") == [["registered", str(len(names))]]
    assert events[-1] == ["END", "testsRun", str(len(names))]
    ignored = {"EXECUTABLE", "PROVIDER", "PROVIDER_IDENTICAL", "COUNT", "END"}
    observed = [(event[0], event[1]) for event in events if event[0] not in ignored]
    registered = [("REGISTER", name) for name in names]
    executed = [(event, name) for name in names for event in ("START", "PASS", "STOP")]
    assert observed == registered + executed
    return {name: [i for i, e in enumerate(events, 1) if e[1] == name] for name in names}


def child_processes(source, folder, process, level, names):
    """Match each workload's actual argv, process group, warning environment and LLVM level."""
    manifest = load(source / "source-manifest.json")
    binaries = [s.split(maxsplit=1) for s in (source / f"sha256-{level}.txt").read_text().splitlines()]
    driver = process["argv"][0]
    assert binaries[0] == [process["executable_sha256"], driver]
    assert binaries[1][0] == manifest["runtime_sha256"]
    records = {path: argv(path) for path in (folder / "audit").glob("*.argv")}
    (outer,) = [p for p, cmd in records.items() if cmd == process["argv"]]
    parent_pid = int(outer.name.split(".")[0])
    assert int(outer.with_suffix(".group").read_text()) == parent_pid
    assert outer.with_suffix(".level").read_text().strip() == "-" + level
    receipts = {}
    for name in names:
        defaults = name.endswith(".test_default_filter_configuration")
        if defaults:
            pure = name.split(".")[-2].startswith("Py")
            options = ["-I", "-c", (BLOCK if pure else "") + DEFAULT]
        else:
            options = ["-Werror::DeprecationWarning", "-c", CONFLICT]
        expected = [driver, "-X", "faulthandler", *options]
        (child,) = [p for p, cmd in records.items()
                    if cmd == expected and p.with_suffix(".parent").read_text().strip() == name]
        pid, clang_pid = map(int, child.name.split(".")[:2])
        assert len({pid, parent_pid, clang_pid}) == 3
        assert int(child.with_suffix(".group").read_text()) == parent_pid
        assert child.with_suffix(".level").read_text().strip() == "-" + level
        environment = sorted(child.with_suffix(".options").read_text().splitlines())
        if not defaults:
            assert environment == ["PYTHONDEVMODE=", "PYTHONWARNINGS=default::DeprecationWarning"]
        receipts[name] = dict(driver_pid=pid, clang_pid=clang_pid, process_group=parent_pid,
                              argv=expected, level="-" + level, warning_options=environment)
    return receipts


def verify(root):
    inventory = root / "original-187.json"
    assert digest(inventory) == INVENTORY
    keys = load(inventory)["keys"]
    names = load(root / "harness/original-keys.json")
    assert len(keys) == 187 and len(set(names)) == len(names) == 4
    assert [name for name, occurrence in keys if name in names and occurrence == 1] == names
    assert digest(root / "harness/upstream/test_warnings/__init__.py") == SUITE
    stock = load(root / "stock-provenance.json")
    assert stock["warnings_omissions"] == 0

    source = root / TAG
    manifest = load(source / "source-manifest.json")
    commit = (source / "merge.commit").read_bytes()
    assert hashlib.sha1(f"commit {len(commit)}\0".encode() + commit).hexdigest() == HEAD
    assert commit.startswith(f'tree {manifest["tree"]}\n'.encode())
    assert manifest["tracked_clean"] and manifest["head"] == manifest["traceback_merge"] == HEAD
    assert manifest["filter_merge"] == "e1d6fcf245c22cf438822623aa5fa385f46ccb86"

    credited = {name: {} for name in names}
    for level in ("stock", "O0", "O3"):
        folder = root / "parents" / ("stock" if level == "stock" else TAG) / level
        process = load(folder / "process.json")
        assert process["exit"] == 0 and not process["timed_out"]
        callbacks = original_callbacks(folder, process, names)
        if level == "stock":
            assert process["executable_sha256"] == stock["stock_executable_sha256"]
            native = {}
        else:
            native = child_processes(source, folder, process, level, names)
        for name in names:
            credited[name][level] = dict(lines=callbacks[name], **native.get(name, {}))

    parents = [dict(id=name, occurrence=1, receipts=data) for name, data in credited.items()]
    return dict(public_head=HEAD, denominator=187, parents=parents)
