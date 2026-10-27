from operator import delitem, length_hint


def current():
    keys = ["retired", "active"]
    cursor = reversed(keys)
    keys.append("future")
    keys[1] = "replacement"
    print(length_hint(cursor))
    for key in cursor:
        print(key)
        break
    keys[0] = "older"
    print(length_hint(cursor))
    print(list(cursor), list(cursor))
    print(length_hint(cursor))

    keys = []
    cursor = reversed(keys)
    print(list(cursor))
    keys.extend(["a", "b", "c"])
    print(list(cursor))

    keys = ["old", "removed"]
    cursor = reversed(keys)
    delitem(keys, -1)
    print(length_hint(cursor))
    keys.append("restored")
    print(length_hint(cursor), list(cursor))
    cursor = reversed(keys)
    delitem(keys, -1)
    print(list(cursor))
    keys.append("late")
    print(list(cursor))

    cursor = reversed([["only reference"]])
    for _ in range(5000):
        discarded = [None]
    print(list(cursor))

    print(list(reversed(("first", "middle", "last"))))
    cursor = reversed((["tuple reference"],))
    for _ in range(5000):
        discarded = [None]
    print(list(cursor))

    class KeyStore:
        def __reversed__(self):
            return reversed(["old", "new"])

    class Disabled:
        __reversed__ = None

    print(list(reversed(KeyStore())))
    for value in (False, set(), Disabled()):
        try:
            reversed(value)
        except TypeError as error:
            print(str(error))
