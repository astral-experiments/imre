value = float("nan")
real = value.real
first = value.imag
second = value.imag
print(real is value, id(real) == id(value))
print(first is second, id(first) == id(second))
print(value in [real])
