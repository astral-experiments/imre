"""Direct product controls, independent of the test fixture transport."""

import importlib as parent
import importlib.machinery
from importlib import machinery
from importlib.machinery import ModuleSpec


class Loader:
    calls: list[str]

    def __init__(self, calls: list[str]):
        self.calls = calls

    def get_source(self, name: str) -> str:
        self.calls.append(name)
        return "source:" + name


class Flag:
    events: list[str]
    passes: bool

    def __init__(self, events: list[str], passes: bool):
        self.events = events
        self.passes = passes

    def __bool__(self) -> bool:
        self.events.append("bool")
        if not self.passes:
            raise ValueError("truth failed")
        return True


def flag(events: list[str], passes: bool) -> Flag:
    events.append("rhs")
    return Flag(events, passes)


def receiver(events: list[str], spec):
    events.append("receiver")
    return spec


def log_text(events: list[str], label: str) -> str:
    events.append(label)
    return label


def parent_identity(prefix: str, code: int) -> None:
    name = prefix + ".child"
    value = machinery.ModuleSpec(name, None).parent
    print(value is prefix)
    print(value is machinery.ModuleSpec(name, None).parent)
    if len(prefix) == 1:
        print(value is chr(code))
        print(value is name[0])


def exercise() -> None:
    parent_identity('', 0)
    parent_identity('a', 97)
    parent_identity('é', 233)
    parent_identity('ÿ', 255)
    parent_identity('Ā', 256)
    parent_identity('aa', 0)
    parent_identity('non_short_identifier', 0)
    parent_identity('not-a-name', 0)
    print(parent.machinery.ModuleSpec("aliased", None).name)
    events: list[str] = []
    loader = Loader(events)
    state = ["initial"]
    spec = importlib.machinery.ModuleSpec(
        loader=loader,
        name=log_text(events, "foobar"),
        is_package=flag(events, True),
        origin=log_text(events, "where"),
        loader_state=state,
    )
    print(repr(events))
    print(spec.loader is loader)
    print(spec.loader.get_source(spec.name))
    print(repr(events))
    print(spec.loader_state is state)
    spec.loader_state.append("through spec")
    print(repr(state))
    print(isinstance(spec, ModuleSpec))
    print(isinstance(spec, tuple))
    alias = spec
    print(alias is spec)
    other = machinery.ModuleSpec("p", loader, loader_state=state)
    print(other is spec)
    print(spec._uninitialized_submodules is other._uninitialized_submodules)
    print(spec.parent is spec.name)
    saved = spec.submodule_search_locations
    if saved is None:
        raise AssertionError("package search locations missing")
    saved.append("from parent")
    print(repr(saved))
    print(spec.submodule_search_locations is saved)
    spec.submodule_search_locations = None
    print(spec.parent)
    spec.submodule_search_locations = saved
    print(spec.parent)
    receiver(events, spec).has_location = flag(events, True)
    print(repr(events))
    spec.has_location = False
    try:
        receiver(events, spec).has_location = flag(events, False)
    except ValueError as exc:
        print(str(exc))
    print(repr(events))
    print(spec.has_location)
    for name in ["", "single", ".x", "x.", "späm.η.🔥", "a..b", "x\x00.ß"]:
        plain = ModuleSpec(name, None)
        package = ModuleSpec(name, None, is_package=1)
        print(repr(plain.parent))
        print(package.parent is name)
    empty = ""
    one = "é"
    print(ModuleSpec("none", None).parent is empty)
    print(ModuleSpec("é.mod", None).parent is one)
    original = "byte\x00code.pyc"
    located = ModuleSpec("mod", None, origin=original)
    print(located.cached is None)
    located.has_location = 7
    print(located.cached is original)
    located.origin = "other.pyc"
    print(located.cached is original)
    located.cached = None
    print(located.cached is located.origin)
    located.origin = ""
    located.cached = None
    print(located.cached is None)
    with_data = ModuleSpec("a", ["data"], origin="", is_package=True)
    print(repr(with_data))
    print(str(with_data))
    print(repr([with_data]))
    print(repr(ModuleSpec("a", None)))


exercise()
