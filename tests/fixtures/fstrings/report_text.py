from dataclasses import dataclass
from textwrap import dedent


@dataclass
class Metric:
    name: str

    def __str__(self) -> str:
        return self.name


def report(label: str, rows: list[Metric]) -> str:
    return dedent(
        f"""
        {label}

          {rows[:2]}
        """
    )


values = [Metric("two\nlines"), Metric("🦀"), Metric("omitted")]
print(repr(report("header", values)))
print(repr(report("header\n  continued", values)))
print(f"{values[:1]!s}|{[1, -2]}|{[[1], [2]]}")
for text in ["", " ", "\n\n", " \tX\n  Y", "  α\n   β\n  ", "\talpha\n\t β"]:
    print(repr(dedent(text=text)))

for text in [
    "  x\r\n \t\u00a0\x1c\r\n   y\r\n",
    "\tx\n \ty\n\u2003\n",
    "\t\u00a0x\n\t x",
    "\n\u2002\n\x85",
    " " * 65_536 + "x",
    "\u2003" * 21_000 + "\n",
]:
    print(repr(dedent(text)))
