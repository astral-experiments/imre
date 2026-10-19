def trim(value: str, chars: str | None) -> str:
    return value.rstrip(chars)


def text(label: str, value: str) -> str:
    print(label)
    return value


for value in [
    "",
    "plain",
    " both \t\n",
    "\x1ccontrol\x1c\x1d\x1e\x1f",
    "wide\u00a0\u202f\u3000",
]:
    print(f">{value.rstrip()}<")
    print(f">{value.rstrip(None)}<")
    print(f">{value.rstrip('')}<")
    print(f">{value.rstrip('ain')}<")

print("abcdabcd".rstrip("dc"))
print("abcabc".rstrip("abc"))
print("é🐍é🐍".rstrip("🐍é"))
print("é🐍x🐍é".rstrip("é🐍"))
print("\0saved\0\0".rstrip("\0"))
print("line\n\n".rstrip("\n"))
print("left\ninside".rstrip("\n"))


print(trim("space \t", None))
print(trim("specificxxx", "x"))


print(text("receiver", "lastxy").rstrip(text("chars", "xy")))
original = "savedxxx"
print(original.rstrip(original := "x"))
print(original)
