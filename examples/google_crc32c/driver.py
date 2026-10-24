"""Run the same public checks and sustained checksum loop on CPython and Relar."""

from sys import argv

from google_crc32c import Checksum, extend, value


def oracle() -> None:
    values = 0
    extends = 0
    print("crc32c-ready")
    while True:
        command = input().split()
        if command[0] == "quit":
            print(f"{values} {extends}")
            return
        payload = bytes([int(octet) for octet in command[2:]])
        if command[0] == "value":
            values += 1
            print(value(payload))
        elif command[0] == "extend":
            extends += 1
            print(extend(int(command[1]), payload))
        else:
            raise ValueError("unknown checksum request")


def measure(operation: str, size: int) -> None:
    payload = b"12345678" * (size // 8)
    print("crc32c-ready")
    while True:
        repetitions = int(input())
        if repetitions == 0:
            return
        crc = 1
        if operation == "extend":
            for _ in range(repetitions):
                crc = extend(crc, payload)
        elif operation == "value":
            for _ in range(repetitions):
                crc = value(payload)
        else:
            raise ValueError("unknown checksum operation")
        print(crc)


def check_public_api() -> None:
    state = Checksum(b"12345")
    clone = state.copy()
    state.update(b"6789")
    assert state.digest() == b"\xe3\x06\x92\x83"
    assert state.hexdigest() == b"e3069283"
    assert clone.digest() != state.digest()
    assert Checksum().digest() == b"\0\0\0\0"
    assert extend(0, b"123456789") == value(b"123456789")
    print("public-api-ok")


if argv[1] == "oracle":
    oracle()
elif argv[1] == "measure":
    measure(argv[2], int(argv[3]))
else:
    check_public_api()
