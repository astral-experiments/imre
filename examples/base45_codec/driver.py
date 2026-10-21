"""A typed file driver for the unchanged upstream Base45 codec."""

from sys import argv

from base45 import b45decode, b45encode


def category(error: Exception) -> str:
    if isinstance(error, UnicodeDecodeError):
        return "UnicodeDecodeError"
    if isinstance(error, KeyError):
        return "KeyError"
    if isinstance(error, ValueError):
        return "ValueError"
    if isinstance(error, TypeError):
        return "TypeError"
    if isinstance(error, AttributeError):
        return "AttributeError"
    return "Exception"


def transform(operation: str, payload: bytes, repetitions: int) -> tuple[bytes, int]:
    result = b""
    checksum = 0
    if operation == "encode":
        for index in range(repetitions):
            result = b45encode(payload)
            checksum += len(result)
            if result:
                checksum += result[0] + result[-1]
    elif operation == "decode-bytes":
        for index in range(repetitions):
            result = b45decode(payload)
            checksum += len(result)
            if result:
                checksum += result[0] + result[-1]
    elif operation == "decode-text":
        text = payload.decode()
        for index in range(repetitions):
            result = b45decode(text)
            checksum += len(result)
            if result:
                checksum += result[0] + result[-1]
    else:
        raise ValueError("unknown codec operation")
    return result, checksum


def main() -> None:
    _, operation, source_path, output_path, repetitions_text = argv
    repetitions = int(repetitions_text)
    if repetitions < 1:
        raise ValueError("repetitions must be positive")
    with open(source_path, "rb") as source:
        payload = source.read()
    try:
        result, checksum = transform(operation, payload, repetitions)
    except (ValueError, KeyError, TypeError, AttributeError) as error:
        print("ERROR")
        print(category(error))
        print(str(error))
        print(error.__suppress_context__)
        cause = error.__cause__
        if cause is None:
            print("None")
            print("")  # noqa: FURB105 - Relar currently requires one print argument.
        else:
            print(category(cause))
            print(str(cause))
    else:
        with open(output_path, "wb") as output:
            output.write(result)
        print("OK")
        print(checksum)


main()
