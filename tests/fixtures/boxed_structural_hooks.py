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
