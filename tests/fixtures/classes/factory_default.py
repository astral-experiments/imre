class Base:
    pass


class Child(Base):
    pass


Alias = Base


def make(factory=Alias) -> None:
    print(factory.__name__, type(factory()) is factory)


make()
make(Child)
