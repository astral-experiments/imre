"""String operations from CPython 3.14.5 Lib/inspect.py (PSF-2.0)."""


def cleandoc(doc: str) -> str:
    lines = doc.expandtabs().split("\n")
    margin = -1
    for line in lines[1:]:
        content = line.lstrip(" ")
        if content:
            indent = len(line) - len(content)
            if margin < 0 or indent < margin:
                margin = indent
    lines[0] = lines[0].lstrip(" ")
    if margin > 0:
        for i in range(1, len(lines)):
            lines[i] = lines[i][margin:]
    first = 0
    last = len(lines)
    while first < last and not lines[first]:
        first += 1
    while first < last and not lines[last - 1]:
        last -= 1
    return "\n".join(lines[first:last])
