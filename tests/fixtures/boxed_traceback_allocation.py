def allocation():
    marker = {}
    marker['start formatter allocation probe 2332'] = 1
    try:
        int('invalid')
    except ValueError as error:
        print(type(error).__name__)
    except MemoryError as error:
        original = error.__context__
        print(type(error).__name__, error.args, type(original).__name__, original.args[0])
    print(int('23'))
