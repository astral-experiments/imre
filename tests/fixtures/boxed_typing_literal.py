from typing import Literal, get_args, get_origin


def current():
    ssl_mode = Literal["adhoc"]
    print(repr(ssl_mode), get_origin(ssl_mode) is Literal, get_args(ssl_mode))
    print(ssl_mode is Literal["".join(("ad", "hoc"))])
    print(Literal[1] == Literal[True], Literal[Literal[1], True, 1].__args__)
    print({Literal[1, 2]: "value"}[Literal[2, 1]])

    class Key:
        def __hash__(self):
            if broken:
                raise ValueError("changed hash")
            return 71

    broken = False
    arg = Key()
    alias = Literal[arg]
    broken = True
    for operation in (lambda: alias == alias, lambda: hash(alias)):
        try:
            operation()
        except ValueError as error:
            print(str(error))
