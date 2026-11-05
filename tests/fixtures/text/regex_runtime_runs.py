from re import compile, findall, split


def scan_text(pattern: str, data: str):
    print(findall(pattern, data))
    print(split(pattern, data))


def scan_bytes(pattern: bytes, data: bytes):
    print(findall(pattern, data))
    print(split(pattern, data))


def through_pattern(pattern, data: str):
    print(pattern.findall(data))
    print(pattern.findall(data, 1, 4))


for text in ["[A-Z_]+", "[^a-z]+", "[-xa]+", "[a-bb-c]+", "(x+)", "x*"]:
    scan_text(text, "A_xxλB\x00👋")
for binary in [b"[a-z]+", b"[^a-z]+", b"[\\x80-\\xff]+"]:
    scan_bytes(binary, b"z\xffab\x00")
through_pattern(compile("[^a-z]+"), "0abλ12")
