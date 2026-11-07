class Outer[T]:
    class Inner:
        value = 7


print(Outer.Inner.__qualname__, Outer.Inner.value)
print(Outer.__parameters__[0] is Outer.__type_params__[0])
