def fields(text: str) -> int:
    columns = text.split()
    text = "changed after splitting"
    return int(columns[2]) - int(columns[1])


def alias(text: str) -> int:
    columns = text.split()
    other = columns
    other[1] = "91"
    return int(columns[1]) + int(columns[2])


def direct(values: list[str]) -> int:
    return int(values.pop().split()[1])


def limited(text: str) -> int:
    columns = text.split()
    return int(columns[0]) + int(columns[1])


for text in [" ignored  30  +42 extra", "a\x1c١_٢\u2003123", "a\v3\f19",
             "a -4611686018427387905 9223372036854775808", "a +0 -0"]:
    print(fields(text))
for text in ["only bad", "a x y", "a 1 7_"]:
    try:
        print(fields(text))
    except IndexError as error:
        print("IndexError " + str(error))
    except ValueError as error:
        print("ValueError " + str(error))
print(alias("a 1 2"))
values = ["spare 7", "use 11"]
print(direct(values))
print(len(values))


# Exercise the default digit limit and tighter interpreter startup settings.
for size in [641, 4301]:
    try:
        print(limited("3 " + "9" * size) > 0)
    except ValueError as error:
        print(str(error))
