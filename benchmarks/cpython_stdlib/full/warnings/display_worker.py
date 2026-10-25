"""Execute the original display tests' actual arguments in a native process."""
import sys
from io import StringIO
from warnings import formatwarning, showwarning


def display(mode: str, message: str, filename: str, lineno: int, line: str | None) -> str:
    if mode == "format":
        return formatwarning(message, Warning, filename, lineno, line)
    result = StringIO()
    showwarning(message, Warning, filename, lineno, result, line)
    return result.getvalue()


def main() -> None:
    line = None if sys.argv[5] == "omitted" else sys.argv[6]
    sys.stdout.write(display(sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), line))


main()
