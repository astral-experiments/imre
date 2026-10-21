from dataclasses import dataclass, field


def helper(value: int) -> int:
    return value + 1


@dataclass
class Names:
    field: int
    helper: int
    len: int
    self: int
    values: list[int] = field(default_factory=list)


item = Names(field=helper(1), helper=3, len=len([4, 5]), self=7)
print(item)
item.values.append(item.field + item.helper + item.len + item.self)
print(item.values[0])
print(helper(item.helper))
print(len(item.values))
