def current():
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
