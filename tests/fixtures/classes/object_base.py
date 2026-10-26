class Root(object):
    def __init__(self, value: int) -> None:
        self.value = value

    def read(self) -> int:
        return self.value


class Child(Root):
    pass


class object(object):
    def read(self) -> int:
        return 43


class ShadowChild(object):
    pass


print(Child(42).read())
print(ShadowChild().read())
