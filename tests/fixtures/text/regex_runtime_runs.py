from re import compile, findall, split


def scan(pattern, data):
    print(findall(pattern, data))
    print(split(pattern, data))


def through_pattern(pattern, data):
    print(pattern.findall(data))
    print(pattern.findall(data, 1, 4))


for pattern in ["[A-Z_]+", "[^a-z]+", "[-xa]+", "[a-bb-c]+", "(x+)", "x*"]:
    scan(pattern, "A_xxλB\x00👋")
for pattern in [b"[a-z]+", b"[^a-z]+", b"[\\x80-\\xff]+"]:
    scan(pattern, b"z\xffab\x00")
through_pattern(compile("[^a-z]+"), "0abλ12")
through_pattern(compile(b"[abc]+"), b"!acab!")
