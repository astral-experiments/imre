class Setting:
    stored: int

    def __init__(self, value: int) -> None:
        self.stored = value

    @property
    def level(self) -> int:
        print(f"get {self.stored}")
        return self.stored

    @level.setter
    def level(self, value: int) -> str:
        print(f"set {value}")
        if value < 0:
            raise ValueError("negative")
        self.stored = value
        return "ignored"


class Inherited(Setting):
    pass


class Doubled(Setting):
    @property
    def level(self) -> int:
        return self.stored

    @level.setter
    def level(self, value: int) -> None:
        self.stored = value * 2


class ReadOnly(Setting):
    @property
    def level(self) -> int:
        return self.stored


def set_via_base(setting: Setting, value: int) -> None:
    setting.level = value


def receiver(item: Inherited) -> Inherited:
    print("receiver")
    return item


def rhs(n: int) -> int:
    print(f"value {n}")
    if n == 0:
        raise RuntimeError("no value")
    return n


class Named:
    stored: str

    def __init__(self) -> None:
        self.stored = ""

    def get_name(self):
        return self.stored

    def set_name(self, name):
        self.stored = name

    name = property(get_name, set_name)


item = Inherited(2)
for n in (4, -1, 0, 3):
    try:
        receiver(item).level = rhs(n)
    except (ValueError, RuntimeError) as error:
        print(str(error))
    print(item.level)

named = Named()
named.name = "first"
print(named.name)
named.set_name("second")
print(named.get_name())
receiver(item).level += rhs(2)
print(item.level)
try:
    receiver(item).level += rhs(0)
except RuntimeError as error:
    print(str(error))
print(item.level)
doubled = Doubled(1)
set_via_base(doubled, 7)
print(doubled.level)
read_only = ReadOnly(3)
try:
    set_via_base(read_only, 17)
except AttributeError as error:
    print(str(error))
print(read_only.level)
