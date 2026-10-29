def lookup_allocation():
    parameter_for_call_storage_with_a_deliberately_long_name = [1, 2]
    handler = MemoryError
    try:
        {"start formatter allocation probe 2332": None}
        value = parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1]
        print("value", value)
    except handler as error:
        print("caught", error.args)
    print("retry", parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1])


def assign_allocation():
    row = [1, 2]
    parameter_for_call_storage_with_a_deliberately_long_name = row
    handler = MemoryError
    try:
        {"start formatter allocation probe 2332": None}
        parameter_for_call_storage_with_a_deliberately_long_name = [3, 4]
        print("value", parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1])
    except handler as error:
        print("caught", error.args, parameter_for_call_storage_with_a_deliberately_long_name is row)
    print("preserved", row == [1, 2])
    parameter_for_call_storage_with_a_deliberately_long_name = row
    print("retry", parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1])
