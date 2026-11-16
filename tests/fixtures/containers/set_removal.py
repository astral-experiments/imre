def take(values: set[int]) -> int:
    return values.pop()


def remove(values: set[int], key: int) -> None:
    return values.remove(key)


values = {1, 2}
alias = values
print(remove(values, 1), alias == {2})
try:
    remove(values, 1)
except KeyError as error:
    print(error.args, alias == {2})

print(take(values), len(alias))
try:
    take(values)
except KeyError as error:
    print(error.args)
