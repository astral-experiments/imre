def keep(text: str, snapshots: list[str]) -> str:
    snapshots.append(text)
    return " kept"


def part(log: list[int], event: int) -> str:
    log.append(event)
    return str(event)


def rebinding() -> None:
    text = "before ordinary assignment"
    text = text + (text := "rebound")
    print(text)
    text = "before augmented assignment"
    text += (text := "rebound") + text
    print(text)


def comprehension_rebinding() -> None:
    text = "before comprehension"
    text += [(text := suffix) for suffix in ["rebound comprehension"]][0]  # noqa: RUF015
    print(text)


snapshots: list[str] = []
text = "original with enough characters to use heap storage"
text += keep(text, snapshots)
print(text)
print(snapshots[0])
text = text + "<" + keep(text, snapshots) + ">"
print(text)
print(snapshots[1])

text += (saved := text)
print(text)
print(saved)
rebinding()
comprehension_rebinding()

suffix = "old suffix"
text = "captured:"
text = text + suffix + (suffix := "new suffix") + suffix
print(text)
print(suffix)

values = ["container-owned heap-sized original string"]
text = values[0]
text += values.pop()
print(text)
print(len(values))

log: list[int] = []
text = "nested:"
text = text + part(log, 1) + (part(log, 2) + part(log, 3))
print(text)
for event in log:
    print(event)
