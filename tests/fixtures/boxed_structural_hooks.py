class Candidate:
    def __eq__(self, other):
        print("compared", other)
        return other == 17


class Boom:
    def __eq__(self, other):
        raise ValueError("sentinel")


def current():
    x = Candidate()
    print([x] == [17])
    print((x,) == (17,))
    print({0: x} == {0: 17})
    print([x] == [x])
    try:
        print([Boom()] == [17])
    except ValueError as error:
        print(type(error).__name__, str(error))
    print([1, 2] == [1, 2], [1, 2] == [1, 3])
    left = []
    right = []

    class Mutate:
        def __eq__(self, other):
            left.pop()
            left.pop()
            right.pop()
            right.pop()
            return True

    left.extend([Mutate(), 3])
    right.extend([17, 4])
    print(left == right, len(left), len(right))
    class Flag:
        def __bool__(self):
            print("truth tested")
            return True

    class Rich:
        def __eq__(self, other):
            return Flag()

    print([Rich()] == [0])

    class Forward:
        def __eq__(self, other):
            print("forward")
            return NotImplemented

    class Reverse:
        def __eq__(self, other):
            print("reverse")
            return True

    print([Forward()] == [Reverse()])
    class BadFlag:
        def __bool__(self):
            raise LookupError("truth sentinel")

    class BadResult:
        def __eq__(self, other):
            return BadFlag()

    try:
        print((BadResult(),) == (1,))
    except LookupError as error:
        print(type(error).__name__, str(error))

    class Recursive:
        def __eq__(self, other):
            return [self] == [other]

    try:
        print([Recursive()] == [0])
    except RecursionError:
        print("recursive hook caught")
    print("recursive hook recovered")

    flags = []

    class MapKey:
        def __init__(self, label):
            self.label = label

        def __hash__(self):
            if flags:
                raise AssertionError("mapping comparison rehashed its key")
            return 17

        def __eq__(self, other):
            print("key comparison", self.label, other.label)
            if "raise" in flags:
                raise ValueError("stored key")
            if "delete" in flags:
                flags.clear()
                del left[left_key]
                for i in range(4096):
                    temporary = [i]
            return True

    left_key = MapKey("left")
    left = {left_key: [7]}
    right = {MapKey("right"): [7]}
    flags.append("cached")
    print(left == right, right != left)
    flags.append("raise")
    try:
        print(left == right)
    except ValueError as exc:
        print("caught", exc)
    flags.pop()
    flags.append("delete")
    print(left == right, len(left), len(right))
    from types import GenericAlias
    print(GenericAlias(list, (Candidate(),)) == GenericAlias(list, (17,)))
    try:
        print(GenericAlias(list, (Boom(),)) == GenericAlias(list, (17,)))
    except ValueError as error:
        print(type(error).__name__, str(error))
