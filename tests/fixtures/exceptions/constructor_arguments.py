class Failure(Exception):
    def __init__(self, value):
        self.value = value


class Repeated(Exception):
    def __init__(self, first, last):
        super().__init__(first)
        self.first = self.args
        super().__init__(last)


class WithMethod(Exception):
    def describe(self) -> str:
        return "extra behavior"


value = Failure(3)
assert value.value == 3 and value.args == (3,)
keyword = Failure(value=4)
assert keyword.value == 4 and keyword.args == ()
repeated = Repeated("first", "last")
assert repeated.first == ("first",) and repeated.args == ("last",)
inherited = WithMethod("details")
assert inherited.args == ("details",)
assert inherited.describe() == "extra behavior"
