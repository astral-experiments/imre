class Defaults:
    text = "déjà " "vu"

    def read(self) -> str:
        return self.text

    @classmethod
    def from_class(cls) -> str:
        return cls.text


class Inherited(Defaults):
    pass


def make(log: list[str], fail: bool) -> Defaults:
    log.append("make")
    if fail:
        raise ValueError("no receiver")
    return Inherited()


def read(value: Defaults) -> str:
    return value.text


log: list[str] = []
print(Defaults.text)
print(Inherited.text)
print(Inherited().read())
print(Inherited.from_class())
print(read(Inherited()))
print(make(log, False).text)
try:
    print(make(log, True).text)
except ValueError as error:
    print(str(error))
print(log)
