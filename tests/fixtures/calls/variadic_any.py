from typing import Any


def count(*values: Any, **named: Any) -> int:
    return len(values) + len(named)


print(count(1, "two", flag=False))
