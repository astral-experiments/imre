def update(values: dict[str, int]) -> set[str]:
    values["late"] = 1
    return {"late", "early"}


a = {"early": 1}
print(repr(sorted(a.keys() & update(a))))
b = {"early": 1}
print(repr(sorted(set(b.keys()) & update(b))))
c = set(b.keys())
alias = set(c)
c.add("extra")
print(repr(sorted(alias)))
print(repr(sorted(c & c)))
