"""Literal spans retain escapes, adjacency, and whitespace independently of fields."""

# ruff: noqa: F541

value = 7
print(f"")  # noqa: FURB105 — an empty f-string is the expression under test
print(f"plain {{braces}}")
print(f"{value}")
print(f"{{{value}}}")
print(f"λ\0🙂 {value}\n尾")
print(f"{value:}|{value!s}|{value!s:}")
print("before " f"{value}" " between " f"{value + 1}" " after")  # fmt: skip
print(r"\uD800" f"{value}" "\n" rf"\uDFFF {value}" " tail")
print(rf"{{raw}}\n{value}\uD800")
print(fr"{value}\t{{end}}")  # fmt: skip
print(f"\\uD800{value}\\U0000DFFF")
print(f"{r'\uD800'}")
print(f"{f'inner:{value}'}:outer")
print(f"""first
{value}
last""")
print(rf"""raw first\n
{value}
raw last\t""")

# CompactString's inline boundary is a storage detail, not a text boundary.
for text in [
    "",
    "12345678901234567890123",
    "123456789012345678901234",
    "1234567890123456789012345",
]:
    print(f"{text}")
    print(f"{text}{value}{text}")
