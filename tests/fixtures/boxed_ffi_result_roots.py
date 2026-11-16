def answer(parameter_for_call_storage_with_a_deliberately_long_name):
    return parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1] - parameter_for_call_storage_with_a_deliberately_long_name[0] - parameter_for_call_storage_with_a_deliberately_long_name[1] + parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1]


def allocation():
    row = [1, 2]
    handler = MemoryError
    # A warm-up would leave enough spare frame roots for the measured call.
    try:
        value = answer(row)
        print("value", value)
    except handler as error:
        print("caught", error.args)
    retry = answer(row)
    assert retry == 3
    print("retry", retry, row == [1, 2])
