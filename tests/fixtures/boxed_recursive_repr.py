def current():
    items = []
    items.append(items)
    print(repr(items), str(items))

    mapping = {}
    mapping["again"] = mapping
    print(mapping)

    link = []
    record = (link,)
    link.append(record)
    print(record, link)

    child = ["child"]
    print([child, child])
    print([child, ValueError(child)])
    cross = []
    parent = {"list": cross}
    cross.append(parent)
    print(parent, cross)

    class Word(str):
        def __repr__(self):
            return "<word>"

    word = Word("text")
    print(repr([word, (word,), {word: word}]), [word])

    class Broken:
        def __repr__(self):
            raise LookupError("rendered")

    try:
        repr([Broken()])
    except LookupError as exc:
        print(exc.args)

    class Key:
        def __repr__(self):
            del mapping[self]
            return repr(mapping)

    key = Key()
    mapping = {key: "saved", "next": 2}
    print(repr(mapping))

    class Member:
        def __hash__(self):
            return 0

        def __repr__(self):
            group.add(4)
            return "member"

    group = {Member(), 1}
    print(repr(group), len(group))


def allocation():
    value = [23]
    for _ in range(18):
        value = [value]
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        repr(value)
    except MemoryError as error:
        print("caught", error.args)
    print(len(repr(value)))
