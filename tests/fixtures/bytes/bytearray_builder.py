def append_checked(buffer: bytearray, value: int) -> None:
    try:
        buffer.append(value)
    except ValueError as error:
        print(str(error))
        print(bytes(buffer))


def build() -> None:
    buffer = bytearray()
    alias = buffer
    print(type(buffer).__name__)
    print(isinstance(buffer, bytearray))
    print(alias is buffer)
    print(buffer is bytearray())
    print(bool(buffer))
    buffer.append(True)
    alias.extend(b"\x00\xff")
    snapshot = bytes(buffer)
    buffer.extend(alias)
    alias.reverse()
    print(len(buffer))
    print(bool(alias))
    print(snapshot)
    print([alias, bytearray()])

    for value in [-1, 256, 1 << 100]:
        append_checked(alias, value)
    other = bytearray()
    other.extend(buffer)
    buffer.append(0)
    print(bytes(other))
    print(bytes(buffer))


build()
