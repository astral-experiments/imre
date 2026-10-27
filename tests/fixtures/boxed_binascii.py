import binascii


def current():
    from binascii import Error, Incomplete, a2b_base64, b2a_base64

    print(type(a2b_base64) is type(len), a2b_base64 is binascii.a2b_base64)
    print(Error.__module__, Error.__name__, Error.__bases__ == (ValueError,))
    print(issubclass(Error, ValueError), Incomplete.__base__ is Exception)
    for data in (b"", b"\x00\xfb\xff", bytearray(b"Python")):
        encoded = b2a_base64(data, newline=False)
        print(encoded, a2b_base64(encoded, strict_mode=True) == bytes(data))
    print(b2a_base64(b"abc"), a2b_base64("Y!WJj"))
    for data in ("é", 42, b"A", b"=YWJj", b"Y!WJj", b"YQ==QQ=="):
        try:
            a2b_base64(data, strict_mode=True)
        except (ValueError, TypeError) as error:
            print(type(error).__name__, str(error), type(error) is Error)
    try:
        a2b_base64(b"A")
    except Error as error:
        print(isinstance(error, ValueError), error.args)
