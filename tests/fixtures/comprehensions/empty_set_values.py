values = {value: set() for value in range(0)}
print(values)

values = {value: set() for value in range(2)}
values[0].add(7)
print(type(values[0]) is set, values)
