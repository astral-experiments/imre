"""A typed file driver around the unchanged, unannotated Luhn library."""

from sys import argv

from luhn import append, checksum, generate, verify


def category(error: Exception) -> str:
    if isinstance(error, UnicodeDecodeError):
        return "UnicodeDecodeError"
    if isinstance(error, ValueError):
        return "ValueError"
    return "Exception"


def transform(operation: str, payload: bytes, repetitions: int) -> tuple[str, int]:
    text = payload.decode()
    total = 0
    number = 0
    valid = False
    result = ""
    if operation == "checksum":
        for index in range(repetitions):
            number = checksum(text)
            total += number
        result = str(number)
    elif operation == "verify":
        for index in range(repetitions):
            valid = verify(text)
            if valid:
                total += 1
        result = str(valid)
    elif operation == "generate":
        for index in range(repetitions):
            number = generate(text)
            total += number
        result = str(number)
    elif operation == "append":
        for index in range(repetitions):
            result = append(text)
            total += len(result)
    else:
        raise ValueError("unknown Luhn operation")
    return result, total


def main() -> None:
    _, operation, source_path, output_path, repetitions_text = argv
    repetitions = int(repetitions_text)
    if repetitions < 1:
        raise ValueError("repetitions must be positive")
    with open(source_path, "rb") as source:
        payload = source.read()
    try:
        result, total = transform(operation, payload, repetitions)
    except ValueError as error:
        print("ERROR")
        print(category(error))
        print(str(error))
        print(error.__suppress_context__)
        cause = error.__cause__
        if cause is None:
            print("None")
            print("")  # noqa: FURB105 - Imre requires one print argument.
        else:
            print(category(cause))
            print(str(cause))
    else:
        with open(output_path, "wb") as output:
            output.write(result.encode())
        print("OK")
        print(total)


main()
