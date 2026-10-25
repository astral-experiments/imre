"""One native heap per real TestCase: constructor, attribute access and mutation."""

import sys
from importlib.machinery import ModuleSpec


class Loader:
    pass


def optional_loader(loader: Loader, present: bool) -> Loader | None:
    return loader if present else None


def text() -> str | None:
    marker = input()
    if marker == "None":
        return None
    if marker != "str":
        raise AssertionError("invalid text marker")
    return input()


def send_text(value: str | None) -> None:
    if value is None:
        print("None")
    else:
        print(repr(value))


def serve() -> None:
    loader = Loader()
    # Use the same homogeneous layout for both None and the real remote loader.
    seed = ModuleSpec("", optional_loader(loader, False))
    specs = [seed]
    while True:
        op = input()
        if op == "quit":
            return
        if op == "new":
            name = input()
            retained = optional_loader(loader, input() == "loader")
            origin = text()
            package = input()
            if package == "default":
                spec = ModuleSpec(name, retained, origin=origin)
            else:
                spec = ModuleSpec(
                    name, retained, origin=origin, is_package=package == "True"
                )
            specs.append(spec)
            print(len(specs) - 1)
        else:
            obj = specs[int(input())]
            if op == "name":
                print(repr(obj.name))
            elif op == "loader":
                obj_loader = obj.loader
                if obj_loader is None:
                    print("None")
                elif obj_loader is loader:
                    print("1")
                else:
                    raise AssertionError("lost loader identity")
            elif op == "origin":
                send_text(obj.origin)
            elif op == "loader_state":
                print(obj.loader_state is None)
            elif op == "submodule_search_locations":
                locations = obj.submodule_search_locations
                if locations is None:
                    print("None")
                else:
                    print(repr(locations))
            elif op == "cached":
                sys.implementation.cache_tag = text()
                sys.pycache_prefix = text()
                send_text(obj.cached)
            elif op == "parent":
                print(repr(obj.parent))
            elif op == "has_location":
                print(obj.has_location)
            elif op == "set_cached":
                obj.cached = text()
                print("None")
            elif op == "set_origin":
                obj.origin = text()
                print("None")
            elif op == "set_has_location":
                obj.has_location = input() == "True"
                print("None")
            else:
                raise AssertionError("unknown protocol operation")
        sys.stdout.flush()


serve()
