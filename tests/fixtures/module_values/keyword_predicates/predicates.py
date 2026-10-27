import typing as t
from keyword import iskeyword as is_python_keyword, issoftkeyword


def hard(key: str) -> bool:
    return is_python_keyword(t.cast(str, key))


def soft(key: str) -> bool:
    return issoftkeyword(key)
