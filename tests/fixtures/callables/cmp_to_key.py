from functools import cmp_to_key


def ascending(left: int, right: int) -> int:
    return left - right


def descending(left: int, right: int) -> int:
    return right - left


def divide(left: int, right: int) -> int:
    return left // right


def observed(log: list[int], value: int) -> int:
    log.append(value)
    return value


def run() -> None:
    key = cmp_to_key(mycmp=ascending)
    other = cmp_to_key(descending)
    a = key(obj=2)
    b = other(3)
    print(key.obj)
    print(a.obj)
    print(a(4).obj)
    print(key("held text").obj)
    print(a < b)
    print(b < a)
    print(a <= key(2))
    print(a == key(2))
    print(a != b)
    print(b > a)
    print(b >= other(3))
    alias = a
    a.obj = 8
    print(a is alias)
    print(a is key(8))
    print(alias.obj)
    print(alias > b)
    calls: list[int] = []
    print(key(observed(calls, 1)) < key(observed(calls, 3)) < key(observed(calls, 2)))
    print(key(observed(calls, 4)) < key(observed(calls, 1)) < key(observed(calls, 9)))
    print(calls)
    try:
        print(hash(a))
    except TypeError as error:
        print(str(error))
    try:
        print(a > 2)
    except TypeError as error:
        print(str(error))
    try:
        print(key < a)
    except AttributeError as error:
        print(str(error))
    fail = cmp_to_key(divide)
    try:
        print(fail(3) < fail(0))
    except ZeroDivisionError as error:
        print(str(error))
    invalid = cmp_to_key(None)
    print(invalid(5).obj)
    try:
        print(invalid(1) == invalid(2))
    except TypeError as error:
        print(str(error))
    try:
        cmp_to_key()
    except TypeError as error:
        print(str(error))
    try:
        key(wrong=1)
    except TypeError as error:
        print(str(error))
    try:
        key(observed(calls, 40), observed(calls, 41))
    except TypeError as error:
        print(str(error))
        print(calls)


def local_comparators() -> None:
    def numeric(left, right):
        print("numeric comparator")
        return int(left) - int(right)

    key = cmp_to_key(mycmp=numeric)
    print(key("kept without comparing").obj)
    print(key(4.0) == key("4"))
    print(key(2) < key("35"))

    def reverse(left, right):
        print("reverse comparator")
        return right - left

    descending_key = cmp_to_key(reverse)
    print(descending_key(9) < key(4))
    print(key(4) < descending_key(9))


def redefined_comparator() -> None:
    def compare(left, right):
        print("first")
        return int(left) - int(right)

    first = cmp_to_key(compare)
    saved = first(2)
    print(saved.obj)
    print(saved < first(4))

    def compare(left, right):
        print("second")
        return int(right) - int(left)

    second = cmp_to_key(compare)
    print(saved < second(4))
    print(second(4) < saved)
    print(first(2.5) < second("4"))
    print(second("4") < first(2.5))


run()
local_comparators()
redefined_comparator()
