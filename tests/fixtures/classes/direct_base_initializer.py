class Parent:
    value: int

    def __init__(self, value: int = 17) -> None:
        self.value = value


class Child(Parent):
    tag: str

    def __init__(self, value: int = 23) -> None:
        Parent.__init__(self, value=value)
        self.tag = "child"


class Grandchild(Child):
    pass


print(Child().value)
print(Grandchild(42).value)
print(Grandchild().tag)


class Forward(Parent):
    tag: str

    def __init__(self, *args, **kwargs):
        Parent.__init__(self, *args, **kwargs)
        self.tag = "forwarded"


class InheritedForward(Forward):
    pass


print(Forward().value)
print(Forward(29).value)
print(InheritedForward(value=31).value)
print(InheritedForward().tag)
