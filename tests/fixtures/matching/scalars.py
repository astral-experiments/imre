def subject(value: int) -> int:
    print("subject")
    return value


def classify(value: int) -> str:
    match subject(value):
        case 0 | -1:
            return "zero or negative one"
        case (2 | 3) as captured:
            return f"small:{captured}"
        case 1000000000000000000000000000000:
            return "large"
        case True:
            return "wrong singleton"
        case other:
            return f"other:{other}"


def boolean_number(value: bool) -> str:
    match value:
        case 1 as captured:
            return f"one:{captured}"
        case 0:
            return "zero"
        case _:
            return "wrong numeric"


def singletons(value: bool | None) -> str:
    match value:
        case None:
            return "none"
        case True:
            return "true"
        case False:
            return "false"
        case _:
            return "wrong singleton"


def optional(value: int | None) -> int:
    match value:
        case None:
            return -1
        case captured:
            return captured + value


def optional_or(value: int | None) -> str:
    match value:
        case (None as captured) | (0 as captured):
            if captured is None:
                return "absent"
            return f"present:{captured}"
        case captured:
            return f"other:{captured}"


def strings(value: str) -> str:
    match value:
        case "é\0🌍":
            return "unicode"
        case "hello" | "world":
            return "greeting"
        case other:
            return other


for value in [-1, 0, 1, 2, 3, 1000000000000000000000000000000]:
    print(classify(value))
print(boolean_number(True))
print(boolean_number(False))
print(singletons(True))
print(singletons(False))
print(singletons(None))
print(optional(None))
print(optional(42))
print(optional_or(None))
print(optional_or(0))
print(optional_or(7))
print(strings("é\0🌍"))
print(strings("hello"))
print(strings("other"))
match None:
    case None as empty:
        print(empty)
