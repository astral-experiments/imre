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


print(Child().value, Grandchild(42).value, Grandchild().tag)
