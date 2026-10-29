def answer(parameter_for_call_storage_with_a_deliberately_long_name):
    return parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1] - parameter_for_call_storage_with_a_deliberately_long_name[0] - parameter_for_call_storage_with_a_deliberately_long_name[1] + parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1]


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
