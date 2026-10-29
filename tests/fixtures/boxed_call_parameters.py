def answer_with_a_deliberately_long_qualified_name(parameter_for_call_storage_with_a_deliberately_long_name):
    return parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1]


def positional_only(parameter_for_call_storage_with_a_deliberately_long_name, /):
    return parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1]


def outer():
    counter = 0

    def enclosed(parameter_for_call_storage_with_a_deliberately_long_name):
        nonlocal counter
        counter = 0
        return parameter_for_call_storage_with_a_deliberately_long_name[0] + parameter_for_call_storage_with_a_deliberately_long_name[1] + counter

    return enclosed


nonlocal_answer = outer()


def allocation(kind="plain"):
    if kind == "posonly":
        answer = positional_only
    elif kind == "nonlocal":
        answer = nonlocal_answer
    else:
        answer = answer_with_a_deliberately_long_qualified_name
    row = [1, 2]
    handler = MemoryError
    marker = {"start formatter allocation probe 2332": None}
    try:
        value = answer(row)
        print("value", value)
    except handler as error:
        print("caught", error.args)
    print("retry", answer(row))
