import copy


class CustomState:
    def __getstate__(self) -> int:
        return 0


try:
    copy.copy(CustomState())
except TypeError as error:
    print(str(error))
