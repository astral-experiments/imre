def current():
    class Text(str):
        pass

    class Mapping(dict):
        pass

    print(str.maketrans is "".maketrans, Text.maketrans is Text("x").maketrans is str.maketrans)
    print(str.__new__(Text, "still a string"))
    original = {"a": "first", 97: "last", "🦊": None, -1: "negative", 1 << 90: "large"}
    table = str.maketrans(original)
    print(list(table.items()), "a🦊b".translate(table), table is not original)
    table[97] = "new"
    print(original["a"], original[97], "a".translate(table))

    marker = []
    table = "".maketrans({Text("é"): marker, True: "one"})
    print(table[233] is marker, list(table)[1] is True, "x".translate(table))
    try:
        "é".translate(table)
    except TypeError as error:
        print(type(error).__name__, str(error))

    for args in (("", ""), ("ä🦊ä", "𐀀aé"), (Text("ä🦊ä"), Text("𐀀aé"), Text("z🦊z"))):
        table = Text.maketrans(*args)
        print(list(table.items()), "ä🦊z".translate(table))

    for args in ((), ({}, {}, {}, {}), (Mapping(),), ({"": 1},), ({"ab": 2},),
                 ({(1,): 2},), (1, "ab"), ("é", "ab"), ("a", None),
                 (None, 1, 2), (None, "b", None)):
        try:
            str.maketrans(*args)
        except (TypeError, ValueError) as error:
            print(type(error).__name__, str(error))
    try:
        str.maketrans(x={})
    except TypeError as error:
        print(type(error).__name__, str(error))
