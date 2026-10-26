# Formatter.get_value is unchanged from CPython 3.14.5 Lib/string/__init__.py.
from _string import formatter_field_name_split

class Formatter:
    def get_value(self, key, args, kwargs):
        if isinstance(key, int):
            return args[key]
        else:
            return kwargs[key]


def check(field: str, override: bool, index: int) -> None:
    key, rest = formatter_field_name_split(field)
    if override:
        key = index
    left = [1]
    right = [2]
    named = [3]
    formatter = Formatter()
    try:
        found = formatter.get_value(key, (left, right), {"label": named})
        print(found is left)
        print(found is right)
        print(found is named)
        found.append(4)
        print(left)
        print(right)
        print(named)
    except (IndexError, KeyError) as error:
        print(repr(error))


for field in ["0", "1", "2", "label", "missing"]:
    check(field, False, 0)
for index in [-1, -2, -3, 10**80, -(10**80)]:
    check("0", True, index)
