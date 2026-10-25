"""Check real aliasing, nested restoration, exceptions and early return."""
from unittest.mock import patch


def leave(mapping: dict[str, str]) -> str:
    with patch.dict(mapping, {"outer": "returning"}, clear=True) as changed:
        return changed["outer"]


mapping = {"before": "original", "kept": "second"}
alias = mapping
scope = patch.dict(mapping, [("before", "positional")], before="keyword", extra="new")
print(scope.stop() is None)
with scope as changed:
    print(changed is alias)
    print(changed["before"])
    print(mapping["extra"])
    try:
        with patch.dict(alias, {"inner": "active"}, clear=True):
            print(mapping["inner"])
            del alias["inner"]
            alias["during"] = "body"
            raise ValueError("inside patch")
    except ValueError as exc:
        print(str(exc))
    print(mapping["before"])
    print("during" in changed)
    del changed["kept"]
print(mapping["before"])
print(mapping["kept"])
print("extra" in alias)
print(leave(mapping))
print(mapping["before"])
active = scope.start()
active["during"] = "body"
print(mapping["during"])
stopped = scope.stop()
if stopped is None:
    raise AssertionError("a started patch must restore its mapping and return False")
print(stopped)
print("during" in alias)
print(scope.stop() is None)
with patch.dict(mapping) as unmodified:
    print(unmodified is mapping)
try:
    patch.dict(mapping, None)
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("explicit None must fail while constructing the patch")

updates = {"before": "construction"}
saved = patch.dict(mapping, updates)
updates["before"] = "after construction"
with saved:
    print(mapping["before"])
print(mapping["before"])
