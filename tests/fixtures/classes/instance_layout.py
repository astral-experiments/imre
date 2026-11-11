"""An annotation or union tag must not erase the concrete class in isinstance."""
class Base:
    pass

class Child(Base):
    pass

class Grandchild(Child):
    pass

class Sibling(Base):
    pass

class Unrelated:
    pass

def view(x: Base) -> Base:
    return x

def direct(x: Base) -> None:
    print(isinstance(x, Child))
    print(isinstance(x, (Child, Unrelated)))
    print(isinstance(x, Base))
    if isinstance(x, Child):
        print("child")
    else:
        print("other")

def optional(x: Base | None) -> None:
    print(isinstance(x, Child))
    print(isinstance(x, Base))

def either(x: Base | Exception) -> None:
    print(isinstance(x, Child))
    print(isinstance(x, (Child, Exception)))
    if isinstance(x, Child):
        print("child")
    else:
        print("other")

def record_read(value: Base, reads: list[int]) -> Base:
    reads.append(1)
    return value

def observe_type() -> None:
    reads: list[int] = []
    observed = type(record_read(Grandchild(), reads))
    print(observed is Grandchild, Child is not observed)
    print(type(record_read(Base(), reads)) is Base, len(reads))

direct(view(Base()))
direct(view(Child()))
direct(view(Grandchild()))
direct(view(Sibling()))
optional(None)
optional(view(Grandchild()))
optional(view(Sibling()))
either(view(Grandchild()))
either(view(Sibling()))
either(ValueError("other"))
observe_type()
