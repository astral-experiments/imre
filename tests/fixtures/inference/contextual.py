def count(values):
    return len(values)


def first(words):
    return words[0] if words else "empty"


def nested(rows):
    return len(rows) + len(rows[0]) if rows else 0


def mapping_size(values):
    return len(values)


def set_size(values):
    return len(values)


def typed_empty(values: list[str]):
    return values


print(count([1, 2]))
print(count([]))
print(first(["hello"]))
print(first([]))
print(nested([["seed"]]))
print(nested([[]]))
print(nested([]))
print(mapping_size({"key": 1}))
print(mapping_size({}))
print(set_size({"first", "second"}))
print(set_size(set()))
print(len(typed_empty([])))
