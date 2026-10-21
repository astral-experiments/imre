def integer(label: str, value: int) -> int:
    print(label)
    return value


def floating(label: str, value: float) -> float:
    print(label)
    return value


def numeric(left: int, right: float) -> float:
    return left + right


def comparison(left: int, right: float) -> bool:
    return left < right


def optional(value: float | None, *, other: float | None = -0.0) -> float | None:
    return value if value is not None else other


print(integer("left", 2) + floating("right", 0.5))
print(floating("left float", 2.5) + integer("right int", 2))
print(
    integer("chain left", 1) < floating("chain middle", 1.5) < integer("chain last", 2)
)
print(
    floating("stopped left", 2.0)
    < integer("stopped middle", 1)
    < floating("must not run", 0.0)
)
print(numeric(right=floating("keyword right", 3.5), left=integer("keyword left", 1)))
print(comparison(9007199254740993, 9007199254740992.0))
print(comparison(9007199254740993, 9007199254740994.0))
saved = optional(None)
if saved is not None:
    print(saved)

huge = int("1" + "0")
for index in range(400):
    huge *= 10
try:
    print(integer("huge left", huge) + floating("right still evaluates", 1.0))
except OverflowError:
    print("mixed conversion failed")
try:
    print(floating("float left", 1.0) * integer("huge right", huge))
except OverflowError:
    print("reverse conversion failed")
print(huge / huge)
print(-huge / huge)
print(huge > float("inf"))
print(huge < float("inf"))
print(huge == float("nan"))
print(huge != float("nan"))
