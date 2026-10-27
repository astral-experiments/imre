"""Method-set setup in Flask mutates a set retained by its caller."""


def current():
    for left_type in (set, frozenset):
        for right_type in (set, frozenset):
            left = left_type(["GET"])
            right = right_type(["GET", "OPTIONS"])
            result = left | right
            print(type(result).__name__, result == {"GET", "OPTIONS"}, result is left)
            before = left
            left |= right
            print(left == result, left is before, before == result, right == result)
            before = left
            left |= left
            print(left == before, left is before)

    marker = []
    class Reflected:
        def __ror__(self, other):
            return marker
    original = {"GET"}
    assigned = original
    assigned |= Reflected()
    print(assigned is marker, original == {"GET"})
    print((original | Reflected()) is marker)
    for invalid in ([], ("OPTIONS",), {"OPTIONS": None}, 4):
        try:
            original |= invalid
        except TypeError as error:
            print(error.args, original == {"GET"})

    class Key:
        def __init__(self, name):
            self.name = name
            self.calls = 0
            self.reject = False

        def __hash__(self):
            self.calls += 1
            if self.calls > 1:
                raise AssertionError("hashed an existing set element")
            return 43

        def __eq__(self, other):
            if self.reject:
                raise ValueError("collision")
            return self.name == other.name

    left_key = Key("equal")
    right_key = Key("equal")
    left = {left_key}
    right = {right_key}
    left |= right
    print(len(left), list(left)[0] is left_key, left_key.calls, right_key.calls)
    copied = set(right)
    print(len(copied), list(copied)[0] is right_key)
    left_key.reject = True
    try:
        left |= right
    except ValueError as error:
        print(error.args, len(left), left_key.calls, right_key.calls)
    frozen = frozenset(right)
    print(len({frozen: True}), right_key.calls)

    class Mutating:
        def __hash__(self):
            return 17

        def __eq__(self, other):
            for i in range(16):
                target.add(i)
            return True

    resident = Mutating()
    incoming = Mutating()
    target = {resident}
    target |= {incoming}
    print(len(target), resident in target, 15 in target)

    class Holder:
        pass
    holder = Holder()
    holder.methods = {"GET"}
    held = holder.methods
    holder.methods |= {"OPTIONS"}
    slots = [held]
    slots[0] |= {"HEAD"}
    print(held is holder.methods, held is slots[0], len(held))
    expected = []
    alias = expected
    expected += ["item"]
    print(expected is alias, alias)

    for view in ({"right": 9}.keys(), {"right": 9}.items()):
        left = frozenset(["left"])
        before = left
        left |= view
        print(type(left).__name__, len(left), before is left, left == view | before)
    left_key = Key("nested")
    right_key = Key("nested")
    left = {frozenset([left_key])}
    right = {frozenset([right_key])}
    left |= right
    print(len(left), left_key.calls, right_key.calls)
