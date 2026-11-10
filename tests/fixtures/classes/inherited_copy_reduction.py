import copy


class Base:
    def __reduce_ex__(self, protocol: int) -> int:
        return 0


class Child(Base):
    pass


try:
    copy.copy(Child())
except TypeError as error:
    print(str(error))
