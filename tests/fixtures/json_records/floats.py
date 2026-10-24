from imre.json import decode


class Number:
    value: float

    def __init__(self, value: float) -> None:
        self.value = value


def repeat(text: str, count: int) -> str:
    result = ""
    for index in range(count):
        result += text
    return result


for token in [
    "0", "-0", "0.0", "-0.0", "1", "-1",
    "9007199254740993", "1e-300", "-1e-999",
    "2.2250738585072012e-308",
    "1.00000000000000011102230246251565404236316680908203125",
    "1.00000000000000011102230246251565404236316680908203126",
    "1.7976931348623157e308",
]:
    record = decode('{"value":' + token + "}", Number)
    print(f'{token} {record.value}')

for token in ["true", "null", '"1.5"', "NaN", "Infinity", "1e309", "-1e309", "1" + repeat("0", 400)]:
    try:
        record = decode('{"value":' + token + "}", Number)
        print(f"unexpected float {record.value}")
    except ValueError:
        print("invalid float")
