def convert(value):
    return 7


def collect(converter, source):
    print("source", source)
    return [converter(source)]


globals()["int"] = convert
print(list(map(int, [(1, 2)])))

globals()["map"] = collect
print(sum(map(int, 42)))
