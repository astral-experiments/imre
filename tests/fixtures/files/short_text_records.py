from typing import TextIO


def projected(stream: TextIO) -> None:
    for line in stream:
        if line.startswith("r "):
            fields = line.split()
            try:
                print(f"{int(fields[1])} {int(fields[2])}")
            except ValueError:
                print("bad number")
            except IndexError:
                print("missing number")
        elif line.endswith("é\n"):
            print("suffix")
        elif not line.startswith("padding"):
            print("other")


def saved_lines(stream: TextIO) -> None:
    first = stream.readline()
    alias = first
    following = stream.readline()
    stream.close()
    try:
        first = stream.readline()
    except ValueError:
        print(alias.startswith("old "))
        print(first.endswith("é\n"))
        print(following.startswith("new "))


with open("records.txt", "w", encoding="utf-8") as out:
    out.write("padding" + "x" * 8178 + "\r\n")
    out.write("r 1 20\rr 12345678 9\nr 1234567890123456789012 3\n")
    out.write("r +12\u2003-4\nr 1_000\t٣\nr one 2\nr 1\n\né\r\n")
    out.write("r 71 82")
with open("records.txt", encoding="utf-8") as inp:
    projected(inp)

for size in [0, 14, 16, 22, 24, 30, 8200, 131073]:
    with open("old-line.txt", "w", encoding="utf-8") as out:
        out.write("old " + "x" * size + "é\nnew 1\n")
    with open("old-line.txt", encoding="utf-8") as inp:
        saved_lines(inp)
