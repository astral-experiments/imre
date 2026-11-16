def answer(row):
    # Repeated interned literals need temporary roots even when the strings exist.
    keys = [
        "repeated_interned_string_literal",
        "repeated_interned_string_literal",
        "repeated_interned_string_literal",
        "repeated_interned_string_literal",
        "repeated_interned_string_literal",
        "repeated_interned_string_literal",
    ]
    return row[keys[0]] + row[keys[1]] + row[keys[2]]


def allocation():
    row = {"repeated_interned_string_literal": 1}
    handler = MemoryError
    assert answer(row) == 3
    marker = {"start formatter allocation probe 2332": None}
    try:
        value = answer(row)
        print("value", value)
    except handler as error:
        print("caught", error.args)
    print("retry", answer(row), row == {"repeated_interned_string_literal": 1})
