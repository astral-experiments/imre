"""Render the literal and name operations from a parsed Jinja Output node."""

from markupsafe._native import escape_text


class TextProgram:
    literals: list[str]
    names: list[str]

    def __init__(self, literals: list[str], names: list[str]) -> None:
        self.literals = literals
        self.names = names

    def render(self, context: dict[str, str]) -> str:
        pieces: list[str] = []
        for i in range(len(self.names)):
            pieces.append(self.literals[i])
            pieces.append(escape_text(context[self.names[i]]))
        pieces.append(self.literals[-1])
        return "".join(pieces)
