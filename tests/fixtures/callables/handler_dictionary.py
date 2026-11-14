def double(n: int) -> int:
    return n * 2


def triple(n: int) -> int:
    return n * 3


handlers = {"selected": double}
alias = handlers
saved = handlers["selected"]
print(handlers["selected"](alias.update({"selected": triple}) or 21))
print(saved(21), handlers["selected"](21), alias is handlers)
