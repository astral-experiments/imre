try:
    ord(memoryview(b"x"))
except TypeError as error:
    print(type(error).__name__, str(error))
else:
    raise AssertionError("ord accepted a memoryview")
