import dataclasses as dc


@dc.dataclass(slots=True)
class Collections:
    values: list[int] = dc.field(default_factory=list)
    labels: dict[str, int] = dc.field(default_factory=dict)
    members: set[str] = dc.field(default_factory=set)
    more: list[int] = dc.field(default_factory=list, kw_only=True)


@dc.dataclass
class OptionalCollection:
    values: list[int] | None = dc.field(default_factory=list)


@dc.dataclass
class OptionalContainers:
    mapping: dict[str, int] | None = dc.field(default_factory=dict)
    members: set[int] | None = dc.field(default_factory=set)


first = Collections()
second = Collections()
first.values.append(3)
first.labels["key"] = 5
first.members.add("member")
first.more.append(7)
print(first)
print(second)
shared = [11]
explicit = Collections(shared, more=shared)
explicit.values.append(13)
print(sum(explicit.more))
print(sum(shared))
print(first == second)
print(OptionalCollection())
print(OptionalCollection(None))
print(OptionalContainers())
print(OptionalContainers(None, None))
