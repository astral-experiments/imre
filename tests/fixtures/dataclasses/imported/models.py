from decorators import record, member, This


@record(slots=True, kw_only=True)
class Row:
    name: str
    values: list[int] = member(default_factory=list)

    @classmethod
    def named(cls, name: str) -> This:
        return cls(name=name)

    @property
    def count(self) -> int:
        return len(self.values)
