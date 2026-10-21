from .leaf import increment


def scale(value, *, factor=2):
    return increment(value) * factor


def compute(values):
    result = 0
    for value in values:
        result += scale(value)
    return result


print("worker initialized")
