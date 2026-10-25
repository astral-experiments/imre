def item(events: list[str], text: str) -> str:
    events.append(text)
    if text == "bad":
        raise ValueError("bad row")
    return text


def report(groups: list[list[str]], early: bool, events: list[str]) -> str:
    lines: list[str] = []
    if early:
        return "early"
    for group in groups:
        lines.append("[")
        lines.extend(item(events, text) for text in group)
        lines.extend(["", "]"])
    return " · ".join(lines)


def visible(early: bool) -> str:
    lines: list[str] = []
    saved = lines
    lines.append("")
    if early:
        return "/".join(saved)
    saved.append(str(len(lines)))
    return "/".join(lines)


def self_extend() -> str:
    lines: list[str] = []
    lines.append("own")
    lines.extend(lines)
    return "-".join(lines)


def read_before_join() -> str:
    lines: list[str] = []
    lines.append("a")
    lines.append("/".join(lines))
    return "→".join(lines)


events: list[str] = []
print(report([], False, events))
print(report([["not evaluated"]], True, events))
print(report([["α", ""], ["🚀"], []], False, events))
try:
    print(report([["before", "bad", "after"]], False, events))
except ValueError as error:
    print(str(error))
for event in events:
    print(event)
print(visible(True))
print(visible(False))
print(self_extend())
print(read_before_join())
