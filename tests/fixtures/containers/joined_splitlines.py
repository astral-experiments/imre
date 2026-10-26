def source(events: list[int], text: str, fail: bool) -> str:
    events.append(1)
    if fail:
        raise ValueError("source")
    return text


def join_two(events: list[int], text: str, other: str, fail: bool) -> str:
    chunks: list[str] = []
    chunks.append("<")
    first = source(events, text, fail).splitlines()
    events.append(2)
    chunks.extend(first[1:])
    second = other.splitlines()
    chunks.extend(second[1:])
    chunks.append(">")
    return "§".join(chunks)


def lines_only(text: str) -> str:
    chunks: list[str] = []
    rows = text.splitlines()
    chunks.extend(rows[1:])
    return "\n".join(chunks)


def seen(text: str) -> str:
    chunks: list[str] = []
    rows = text.splitlines()
    alias = chunks
    chunks.extend(rows[1:])
    alias.append(str(len(chunks)))
    return "\n".join(chunks)


def rebind(text: str) -> str:
    chunks: list[str] = []
    rows = text.splitlines()
    chunks.extend(rows[1:])
    rows = ["one", "two", "three"]
    chunks.extend(rows[1:])
    return "\n".join(chunks)


def before_rebind(out: list[str], text: str) -> str:
    rows = text.splitlines()
    out.extend(rows[1:])
    out = []
    out.extend(["only"])
    return "\n".join(out)


visible: list[str] = []
print(before_rebind(visible, "h\na\nb"))
print(len(visible))
events: list[int] = []
for text in [
    "",
    "\n",
    "header",
    "header\n",
    "header\n\n",
    "header\n\n\nx",
    "header\r\nα\x0b\x1eβ\u0085\u2028𝄞\u2029",
    "header\nlast\n",
]:
    print(join_two(events, text, "header\nB\nC", False))
    print("[" + lines_only(text) + "]")
print(seen("h\na\nb\nc"))
print(rebind("h\n1\n2"))
try:
    print(join_two(events, "header\nunused", "other", True))
except ValueError as error:
    print(str(error))
for event in events:
    print(event)
