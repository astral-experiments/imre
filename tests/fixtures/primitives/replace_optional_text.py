def replace(old: str | None, new: str | None) -> str:
    return "xx".replace(old, new)


print(replace("x", "y"))
try:
    replace("x", None)
except TypeError as error:
    print(error)
