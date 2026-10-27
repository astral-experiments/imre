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
