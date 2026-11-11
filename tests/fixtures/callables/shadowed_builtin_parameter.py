def use(str, value):
    return str(value)


def alternate(value):
    print("selected", value)
    return "rebound"


print(use(alternate, 3))
print(use(value=5, str=alternate))
