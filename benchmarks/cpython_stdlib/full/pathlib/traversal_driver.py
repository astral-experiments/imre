"""Keep actual native path and iterator objects alive across upstream fixture calls."""
from pathlib import Path
import sys


def arg(i: int) -> str:
    with open(sys.argv[1] + "/" + str(i), "rb") as f:
        return f.read().decode()


def case_option(text: str) -> bool | None:
    if text == "none":
        return None
    return text == "true"


def intern(paths: list[Path], lookup: dict[Path, int], value: Path) -> int:
    if value in lookup:
        return lookup[value]
    number = len(paths)
    paths.append(value)
    lookup[value] = number
    return number


paths = [Path(".")]
lookup = {paths[0]: 0}
# The seed establishes the concrete cursor type without scanning any directory.
cursors = [paths[0].glob("*")]
print("READY")
while True:
    command = input()
    if command == "quit":
        break
    try:
        if command == "new":
            value = Path()
            for i in range(int(arg(0))):
                value = value.joinpath(arg(i + 1))
            print(intern(paths, lookup, value))
        elif command == "join":
            value = paths[int(arg(0))]
            for i in range(int(arg(1))):
                value = value.joinpath(arg(i + 2))
            print(intern(paths, lookup, value))
        elif command == "text":
            print(repr(str(paths[int(arg(0))])))
        elif command == "equal":
            print(paths[int(arg(0))] == paths[int(arg(1))])
        elif command == "parent":
            print(intern(paths, lookup, paths[int(arg(0))].parent))
        elif command == "count_part":
            print(paths[int(arg(0))].parts.count(arg(1)))
        elif command == "exists":
            print(paths[int(arg(0))].exists())
        elif command == "mkdir":
            paths[int(arg(0))].mkdir(mode=int(arg(1)), parents=arg(2) == "true", exist_ok=arg(3) == "true")
            print(None)
        elif command == "iterdir":
            cursors.append(paths[int(arg(0))].iterdir())
            print(len(cursors) - 1)
        elif command == "glob" or command == "rglob":
            root = paths[int(arg(0))]
            sensitive = case_option(arg(2))
            follow = arg(3) == "true"
            if arg(4) == "path":
                pattern_path = paths[int(arg(1))]
                if command == "glob":
                    cursors.append(root.glob(pattern_path, case_sensitive=sensitive, recurse_symlinks=follow))
                else:
                    cursors.append(root.rglob(pattern_path, case_sensitive=sensitive, recurse_symlinks=follow))
            else:
                pattern = arg(1)
                if command == "glob":
                    cursors.append(root.glob(pattern, case_sensitive=sensitive, recurse_symlinks=follow))
                else:
                    cursors.append(root.rglob(pattern, case_sensitive=sensitive, recurse_symlinks=follow))
            print(len(cursors) - 1)
        elif command == "iter":
            cursor = cursors[int(arg(0))]
            print(iter(cursor) is cursor)
        elif command == "next":
            print(intern(paths, lookup, next(cursors[int(arg(0))])))
        else:
            raise RuntimeError("unknown native traversal command")
    except StopIteration:
        print("!StopIteration")
    except ValueError as error:
        print("!ValueError " + repr(str(error)))
    except NotImplementedError as error:
        print("!NotImplementedError " + repr(str(error)))
    except FileNotFoundError as error:
        print("!FileNotFoundError " + repr(str(error)))
    except NotADirectoryError as error:
        print("!NotADirectoryError " + repr(str(error)))
    except PermissionError as error:
        print("!PermissionError " + repr(str(error)))
    except OSError as error:
        print("!OSError " + repr(str(error)))
