from _string import formatter_field_name_split as split
from _string import formatter_parser as parse


def source() -> str:
    return "-".join(["dynamic", "name", "retained"])


s = source()
p = parse(s)
q = parse(s)
print(p is q)
print(next(p)[0] is s)
first, tail = split(s)
if isinstance(first, str):
    print(first is s)
print(next(tail, None) is None)
alias = iter(q)
print(q is alias)
s = "replaced"
print(next(alias))
print(next(q, None) is None)
print(list(parse("a{}b{{}}")))
component, rest = split("x.č[٠٢].y[00]")
print(list(rest))
a = parse("z{left!r:{rank}}")
row = next(a)
for ignored in parse("xx{second}"):
    print(ignored)
print(row)
