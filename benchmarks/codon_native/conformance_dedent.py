"""Native Codon implementation of the reporter's textwrap.dedent import."""


def _blank(line: str) -> bool:
    # CPython also treats the four ASCII information separators as whitespace.
    return all(char.isspace() or char in "\x1c\x1d\x1e\x1f" for char in line)


def dedent(text: str) -> str:
    lines = text.split("\n")
    nonblank = [line for line in lines if not _blank(line)]
    margin = 0
    if nonblank:
        first, last = min(nonblank), max(nonblank)
        while first[margin] in " \t" and first[margin] == last[margin]:
            margin += 1
    return "\n".join("" if _blank(line) else line[margin:] for line in lines)
