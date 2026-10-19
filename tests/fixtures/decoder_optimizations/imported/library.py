TABLE = {"a": 0, "b": 1 << 130, "é": -17}


def translate(text: str) -> list[int]:
    return [TABLE[character] for character in text]


def translate_bytes(data: bytes) -> list[int]:
    return [TABLE[character] for character in data.decode()]


def source() -> str:
    TABLE["a"] += 1
    return "aaé"


def mutated_source() -> list[int]:
    return [TABLE[character] for character in source()]


def byte_source(fail: bool) -> bytes:
    TABLE["a"] += 1
    if fail:
        return b"x\xff"
    return b"aa\xc3\xa9"


def mutated_bytes_source(fail: bool) -> list[int]:
    return [TABLE[character] for character in byte_source(fail).decode()]
