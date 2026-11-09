import operator
from operator import is_not


class A:
    x: int

    def __init__(self) -> None:
        self.x = 1


class B:
    x: int

    def __init__(self) -> None:
        self.x = 1


def exact_str(value: object) -> bool:
    return operator.is_(type(value), str)


def not_exact_tuple(value: object) -> bool:
    return is_not(tuple, type(value))


def require_text(value: str) -> str:
    return value


def guarded_text(value: object) -> str:
    if is_not(str, type(value)):
        return "fallback"
    return require_text(value)


print(guarded_text("text"))
print(guarded_text(1))
print(operator.is_(A, A))
print(operator.is_(A, B))
print(is_not(A, A))
print(is_not(A, B))
print(exact_str("text"))
print(exact_str(1))
print(not_exact_tuple((1, 2)))
print(not_exact_tuple("text"))
