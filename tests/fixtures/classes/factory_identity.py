class Base:
    @classmethod
    def check(cls) -> None:
        saved: type[Base] = cls
        print(isinstance(saved, type), type(saved()) is saved, saved.__name__)


class Child(Base):
    pass


def is_class(value: type[Base] | None = None) -> bool:
    return isinstance(value, type)


Base.check()
Child.check()
print(is_class(), is_class(Child))


def check_target(saved: type[Base]) -> None:
    type = int
    print(isinstance(saved, type))


check_target(Child)
