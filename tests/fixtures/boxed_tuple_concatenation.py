def concatenate(left, right):
    return left + right


def allocation():
    first, last = object(), object()
    left, right = (first,) * 1025, (last,) * 1028
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        concatenate(left, right)
    except MemoryError as error:
        print("caught", error.args)
    result = concatenate(left, right)
    print(len(left), len(right), len(result), result[0] is first, result[-1] is last)
