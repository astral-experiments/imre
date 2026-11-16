def stamp(n):
    print("default", n)
    return n


def factory(seed):
    callback = lambda value=stamp(seed): print("call", value, seed)
    seed += 10
    return callback


first = factory(2)
second = factory(7)
first()
second(99)
print(first is second, first.__code__ is second.__code__)
