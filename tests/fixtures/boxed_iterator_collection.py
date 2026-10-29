def allocation(kind):
    row = [1] * 513
    source = filter(None, row)
    constructor = list if kind == "list" else tuple
    handler = MemoryError
    marker = {"start formatter allocation probe 2332": None}
    try:
        result = constructor(source)
        print("value", len(result), result[0] == result[-1] == 1)
    except handler as error:
        print("caught", error.args)
    print("retry", len(constructor(filter(None, row))))
