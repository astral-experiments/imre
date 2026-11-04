def earlier(code: int, other: str) -> bool:
    return chr(code) < other


print(earlier(0xD800, "\ue000"))
print(earlier(0xDFFF, "\U00010000"))
print(earlier(0x1F642, "\ue000"))
