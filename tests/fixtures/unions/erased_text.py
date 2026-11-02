def require_text(value: str) -> str:
    return value


def selected(value: object, default: str) -> str:
    if type(value) is str:
        return require_text(value)
    return default


def reversed_guard(value: object, default: str) -> str:
    if str is not type(value):
        return default
    return require_text(value)


def pair(first: object, second: object) -> bool:
    if type(first) is str and type(second) is str:
        return require_text(first) == require_text(second)
    return False


def copied_value(text: str) -> str:
    mapping: dict[str, object] = {"key": text}
    result = selected(mapping["key"], "wrong")
    mapping["key"] = (1, 2)
    assert selected(mapping["key"], "missing") == "missing"
    return result


def conditional(value: object) -> str:
    return require_text(value) if type(value) is str else str(value)


def after_boolean(value: object) -> str:
    matched = type(value) is str and require_text(value) == "match"
    if matched:
        return "matched"
    return str(value)


original = "".join(["separate ", "allocation"])
assert selected(original, "wrong") is original
assert reversed_guard(original, "wrong") is original
assert copied_value(original) is original
assert pair(original, original)
assert not pair(original, "unequal")
assert not pair(original, (1, 2))
assert reversed_guard(b"bytes", "default") == "default"
assert conditional(42) == "42"
assert conditional(original) is original
assert after_boolean(42) == "42"
assert after_boolean("match") == "matched"
