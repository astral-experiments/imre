def values():
    value = float("nan")
    yield value
    yield value.real


stream = values()
value = next(stream)
alias = next(stream)
print(value in [alias], value in [float("nan")])
print(value in (alias,), value in (float("nan"),))
