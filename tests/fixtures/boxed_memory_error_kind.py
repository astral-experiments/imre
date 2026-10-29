def answer(row):
    return row[0] + row[1]


def allocation():
    row = [1, 2]
    handler = MemoryError
    assert answer(row) == 3
    marker = {"start formatter allocation probe 2332": None}
    try:
        value = answer(row)
        print("value", value)
    except handler as error:
        print("caught", error.args)
    print("retry", answer(row), row == [1, 2])
