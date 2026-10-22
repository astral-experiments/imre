"""Native UTF-8/replacement urllib.parse.unquote for the comparison programs."""


def _hexadecimal(value: int) -> int:
    if 48 <= value <= 57:
        return value - 48
    if 65 <= value <= 70:
        return value - 65 + 10
    if 97 <= value <= 102:
        return value - 97 + 10
    return -1


def unquote(string: str) -> str:
    if "%" not in string:
        return string
    source = string.encode("utf-8")
    result = bytearray()
    index = 0
    while index < len(source):
        if source[index] == 37 and index + 2 < len(source):
            high = _hexadecimal(source[index + 1])
            low = _hexadecimal(source[index + 2])
            if high >= 0 and low >= 0:
                result.append(high * 16 + low)
                index += 3
                continue
        result.append(source[index])
        index += 1
    return bytes(result).decode("utf-8", "replace")
