"""Generate the raw decomposition table using the pinned Unicode 16 UnicodeData.txt."""

from hashlib import sha256
from pathlib import Path
import sys


if __name__ == "__main__":
    data = Path(sys.argv[1]).read_bytes()
    if sha256(data).hexdigest() != "ff58e5823bd095166564a006e47d111130813dcf8bf234ef79fa51a870edb48f":
        raise SystemExit("Expected UnicodeData.txt for Unicode 16.0.0")
    root = Path(__file__).resolve().parents[1]
    table = root / "crates/imre-runtime/native/ucd16-decomposition.txt"
    with table.open("w", encoding="ascii", newline="\n") as output:
        for line in data.decode("ascii").splitlines():
            fields = line.split(";")
            if fields[5]:
                print(fields[0], fields[5], file=output)
