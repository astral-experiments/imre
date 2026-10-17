"""Local roots may be borrowed; removable list elements must remain owned."""


def first(value: int, ignored: int) -> int:
    return value


def drain(items: list[int]) -> int:
    items.pop()
    return 0


def replace(items: list[int]) -> int:
    items[0] = 7
    return 3


def replace_index(items: list[int]) -> int:
    items[0] = 0
    return 0


def replace_upper(items: list[int]) -> int:
    items[0] = 0
    return 1000000000000000000000000000002


def identity(items: list[int]) -> list[int]:
    return items


def integer_identity(value: int) -> int:
    return int(value)


def integer_after_rebind() -> int:
    value = 1000000000000000000000000000000
    saved = int(value)
    value = 0
    return saved


def string_identity(value: str) -> str:
    return value


def conditional_rebind(items: list[int], flag: bool) -> list[int]:
    if flag:
        items = [2000000000000000000000000000000]
    return items


def loop_rebind(value: int) -> int:
    for value in range(2):
        pass
    return value


def while_rebind(items: list[int], flag: bool) -> list[int]:
    while flag:
        items = [3000000000000000000000000000000]
        flag = False
    return items


def same_list(left: list[int], right: list[int]) -> int:
    saved = left[0]
    right.pop()
    left.append(5)
    return saved


def rebind_argument(items: list[int]) -> int:
    items = [0]
    return len(items)


def length_after(items: list[int], ignored: int) -> int:
    return len(items)


def append_count(items: list[int]) -> int:
    items.append(2000000000000000000000000000000)
    return 2


def main() -> None:
    # The first argument's integer initially has no owner outside this list.
    items = [1000000000000000000000000000000]
    print(first(items[0], drain(items)))
    print(len(items))

    # An ordinary local is a separate root even when the source element is removed.
    items = [1000000000000000000000000000000]
    local = items[0]
    print(first(local, drain(items)))
    print(local)
    items = [1000000000000000000000000000000]
    print(items[0] + drain(items))

    # Augmented assignment captures its old item before evaluating the RHS.
    items = [1000000000000000000000000000000]
    items[0] += replace(items)
    print(items[0])

    # Ordinary assignment captures its RHS before evaluating the target index.
    items = [1000000000000000000000000000000]
    items[replace_index(items)] = items[0]
    print(items[0])

    # A comparison chain keeps its middle item alive across the next operand.
    items = [1000000000000000000000000000000]
    print(0 < items[0] < replace_upper(items))
    print(items[0])
    items = [1000000000000000000000000000000]
    print(0 > items[0] < replace_upper(items))
    print(items[0])

    # Assignment, identity conversion, and return must acquire owned references.
    items = [1000000000000000000000000000000]
    alias = items
    items = []
    alias = alias
    print(alias[0])
    value = integer_identity(1000000000000000000000000000000)
    value = value
    value = int(value)
    print(value)
    print(integer_after_rebind())
    text = string_identity("retained string λ")
    text = text
    print(text)
    print(identity([1000000000000000000000000000000])[0])

    # Rebinding analysis must visit conditional branches and both loop forms.
    print(conditional_rebind([1000000000000000000000000000000], False)[0])
    print(conditional_rebind([1000000000000000000000000000000], True)[0])
    print(loop_rebind([1000000000000000000000000000000].pop()))
    print(while_rebind([1000000000000000000000000000000], True)[0])
    print(while_rebind([1000000000000000000000000000000], False)[0])

    # Unchanged parameter bindings may still refer to the same mutable object.
    items = [1000000000000000000000000000000]
    print(same_list(items, items))
    print(items[0])

    # A callee can rebind its own parameter but cannot replace the caller's root.
    print(length_after(items, rebind_argument(items)))
    print(items[0])

    # Borrow the list object, not a snapshot or a borrow of its resizable buffer.
    copies = items * append_count(items)
    print(len(copies))
    print(copies[1])
    print(copies[3])


main()
