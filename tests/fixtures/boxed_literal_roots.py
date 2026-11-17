import sys


def answer(row):
    # Repeated interned literals must stay rooted while their cold slot is loaded.
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
    key = sys.intern("".join(("repeated_", "interned_", "string_literal")))
    row = {key: 1}
    handler = MemoryError
    assert row[key] == 1
    marker = "start formatter allocation probe 2332"
    try:
        value = answer(row)
        print("value", value)
    except handler as error:
        print("caught", error.args)
    print("retry", answer(row), row == {key: 1})
