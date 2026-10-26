import copy
from collections import defaultdict
from http import HTTPStatus


def maybe(value: list[int] | None) -> list[int] | None:
    return copy.deepcopy(value, memo=None)


def check_alias_pattern(seed: int) -> None:
    leaves: list[list[int]] = []
    for i in range(5):
        leaves.append([seed, i])
    selected: list[list[int]] = []
    for i in range(7):
        selected.append(leaves[((seed >> i) + i) % len(leaves)])
    graph = (selected, {"both": selected})
    copied = copy.deepcopy(graph)
    assert copied[0] is copied[1]["both"]
    assert copied[0] is not selected
    for i in range(7):
        assert copied[0][i] is not selected[i]
        for j in range(i):
            assert (copied[0][i] is copied[0][j]) == (selected[i] is selected[j])
    copied[0][0].append(-1)
    assert len(selected[0]) == 2
    for i in range(7):
        assert len(copied[0][i]) == (3 if selected[i] is selected[0] else 2)


def main() -> None:
    for seed in range(257):
        check_alias_pattern(seed)
    leaf = [1 << 120]
    wrapped = [leaf, leaf]
    mapping = {"a": wrapped, "b": wrapped}
    outer = (leaf, mapping, wrapped)
    result = copy.deepcopy(memo=None, x=outer)
    rleaf, rmap, rwrap = result
    print(rleaf is leaf)
    print(rmap is mapping)
    print(rwrap is wrapped)
    print(rleaf is rwrap[0])
    print(rwrap[0] is rwrap[1])
    print(rmap["a"] is rwrap)
    print(rmap["a"] is rmap["b"])
    leaf.append(1)
    rleaf.append(2)
    del rmap["b"]
    print(repr(leaf))
    print(repr(rmap))
    print(len(mapping))
    clone = copy.deepcopy(outer)
    print(clone[0] is rleaf)
    print(clone[1]["a"][1] is clone[0])
    nullable = maybe(leaf)
    if nullable is not None:
        print(nullable is leaf)
        print(repr(nullable))
    print(maybe(None) is None)
    assert copy.deepcopy(HTTPStatus.OK) is HTTPStatus.OK
    assert copy.deepcopy([HTTPStatus.OK])[0] is HTTPStatus.OK
    s = {("x", 2), ("y", 3)}
    sets = [s, s]
    copied = copy.deepcopy(sets)
    print(copied[0] is copied[1])
    copied[0].add(("z", 0))
    print(len(s))
    print(len(copied[1]))
    d: defaultdict[str, list[int]] = defaultdict(list)
    d["x"] = leaf
    cp = copy.deepcopy(d)
    cp["x"].append(99)
    print(repr(d["x"]))
    cp["fresh"].append(3)
    print("fresh" in d)
    print(repr(cp["fresh"]))

main()
