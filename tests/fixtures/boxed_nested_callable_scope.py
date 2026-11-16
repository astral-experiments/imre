def outer():
    x = 10

    class C:
        x = 20

        def read(self):
            return lambda: (x, __class__.x)

    read = C().read()
    x = 30
    return read

print(outer()())
