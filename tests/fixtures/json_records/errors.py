from relar.json import decode


class Item:
    name: str
    amount: int

    def __init__(self, name: str, amount: int = 0) -> None:
        self.name = name
        self.amount = amount


class Report:
    total: int
    items: list[Item]

    def __init__(self, total: int, items: list[Item] | None = None) -> None:
        self.total = total
        self.items = [] if items is None else items


def repeat(text: str, count: int) -> str:
    result = ""
    for index in range(count):
        result += text
    return result


invalid = [
    "", "null", "[]", "{}", '{"total":null}', '{"total":true}',
    '{"total":1.0}', '{"total":1e0}', '{"total":"1"}',
    '{"total":01}', '{"total":NaN}', '{"total":Infinity}',
    '{"total":1,}', '{"total":1} trailing', '{"total":1} {}',
    '{"total":1,"total":2}', '{"total":1,"to\\u0074al":2}',
    '{"total":1,"items":null}', '{"total":1,"items":{}}',
    '{"total":1,"items":[null]}', '{"total":1,"items":[{}]}',
    '{"total":1,"items":[{"name":false}]}',
    '{"total":1,"items":[{"name":"\\ud800"}]}',
    '{"total":1,"items":[{"name":"ok","amount":null}]}',
    '{"total":1,"items":[{"name":"ok","name":"again"}]}',
    '{"total":1,"ignored":[1,]}',
    '{"total":1,"items":[{"name":"\x00"}]}',
    '{"total":1,"\\ud800":0}',
]
for index, text in enumerate(invalid):
    try:
        result = decode(text, Report)
        print(f"unexpected success {index} {result.total}")
    except ValueError:
        print(f"invalid {index}")

# Repeated unknown keys are intentionally ignored, including ignored subtrees.
unknown = decode('{"total":1,"ignored":false,"ignored":{"same":1,"same":2}}', Report)
print(f"unknown {unknown.total}")
huge = decode('{"total":' + repeat("9", 5000) + ',"ignored":1e9999}', Report)
print(huge.total % 1000000007)
ignored_text = decode('{"total":5,"ignored":"\\ud800"}', Report)
print(f"ignored text {ignored_text.total}")
quoted = decode('{"total":4,"ignored":"' + repeat("[", 200) + '"}', Report)
print(f"quoted depth {quoted.total}")
for depth in [127, 128]:
    text = '{"total":2,"ignored":' + repeat("[", depth) + "0" + repeat("]", depth) + "}"
    try:
        result = decode(text, Report)
        print(f"depth accepted {depth} {result.total}")
    except ValueError:
        print(f"depth rejected {depth}")

# Escaped quotes keep bracket text inside the string; an escaped backslash
# immediately before its closing quote must still end the quoted span.
quoted_noise = r'"quote: \" slash-quote: \\\" ' + repeat("[{", 200) + r' closing: ]} tail: \\"'
for quoted_depth in [127, 128]:
    text = '{"total":6,"noise":' + quoted_noise + ',"ignored":' + repeat("[", quoted_depth) + quoted_noise + repeat("]", quoted_depth) + ',"after":"done"}'
    try:
        result = decode(text, Report)
        assert quoted_depth == 127
        print(f"quoted depth accepted {quoted_depth} {result.total}")
    except ValueError:
        assert quoted_depth == 128
        print(f"quoted depth rejected {quoted_depth}")

# Unknown scalars and containers follow the same depth and syntax rules after
# successfully decoding known strings. The nesting error wins over an early type error.
for ignored in ['null', 'true', '17', '1e9999', '"escaped\\ntext"', '"\\ud800"', '{"k":[0]}']:
    named = decode('{"name":"shared","ignored":' + ignored + '}', Item)
    assert named.name == 'shared'
for depth in [127, 128]:
    nested = repeat("[", depth) + "0" + repeat("]", depth)
    try:
        named = decode('{"name":"shared","ignored":' + nested + '}', Item)
        assert depth == 127 and named.name == "shared"
    except ValueError:
        assert depth == 128
try:
    named = decode('{"name":false,"ignored":' + repeat("[", 128) + '0' + repeat("]", 128) + '}', Item)
    print("unexpected late depth success")
except ValueError as error:
    print(str(error).startswith("JSON nesting exceeds 128"))

for length in [127, 128, 129]:
    try:
        named = decode(repeat("[", length), Item)
        print("unexpected malformed success")
    except ValueError as error:
        assert str(error).startswith("JSON nesting exceeds 128") == (length > 128)

shared = decode(r'["shared","shared","\u0073hared","different","additional tag","shared"]' + repeat(" ", 512), list[str])
changed = shared[0]
changed += "!"
shared[0] = changed
print(f"{shared[0]} {shared[1]} {shared[2]} {shared[3]} {shared[5]}")

# Every failure occurs after owning long strings, BigInts, nested records, and lists.
prefix = '{"total":1267650600228229401496703205376,"items":[{"name":"' + repeat("x", 1024) + '","amount":1267650600228229401496703205376},'
for attempt in range(25):
    try:
        result = decode(prefix + '{"name":"late","amount":false}]}', Report)
        print(f"unexpected recovery {result.total}")
    except ValueError:
        pass
    recovered = decode('{"total":3,"items":[{"name":"recovered"}]}', Report)
    if attempt == 24:
        print(f'{recovered.total} {recovered.items[0].name}')
