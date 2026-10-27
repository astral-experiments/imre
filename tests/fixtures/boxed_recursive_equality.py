def current():
    left = []
    right = []

    class Clear:
        def __eq__(self, other):
            left.clear()
            right.clear()
            return False

    left.append(Clear())
    right.append(1)
    assert left == right

    class Grow:
        def __eq__(self, other):
            left.append(1)
            return True

    class Later:
        def __eq__(self, other):
            raise ValueError("later comparison")

    left.extend([Grow(), Later()])
    right.extend([1, 2])
    try:
        left == right
    except ValueError as error:
        assert str(error) == "later comparison"
    else:
        raise AssertionError("the next common element was skipped")

    left = []
    right = []
    left.append(left)
    right.append(right)
    for operator in range(3):
        try:
            if operator == 0:
                print(left == right)
            elif operator == 1:
                print(left != right)
            else:
                print(left in [right])
        except RecursionError:
            print("recursive", operator)
    print(left == left, left in [left])
    left = []
    right = []
    left.append({"entry": (left,)})
    right.append({"entry": (right,)})
    try:
        print(left == right)
    except RecursionError:
        print("recursive mapping")
    right[0] = {"other": ()}
    print(left != right)
    shared = [1]
    print([shared, shared] == [[1], [1]])
    a = 1
    b = 1
    for _ in range(12000):
        a = [a]
        b = [b]
    print(a == b)
