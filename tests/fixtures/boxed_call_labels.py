def answer_with_a_deliberately_long_qualified_name(row):
    return row[0] + row[1]


def allocation():
    row = [1, 2]
    handler = MemoryError
    marker = {"start formatter allocation probe 2332": None}
    try:
        value = answer_with_a_deliberately_long_qualified_name(row)
        print("value", value)
    except handler as error:
        print("caught", error.args)
    print("retry", answer_with_a_deliberately_long_qualified_name(row))
