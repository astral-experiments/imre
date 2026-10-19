from typing import BinaryIO, TextIO


def text_truth(stream: TextIO | None) -> None:
    print(bool(stream))
    if stream:
        print(stream.closed)


def binary_truth(stream: BinaryIO | None) -> None:
    print(bool(stream))
    if stream:
        print(stream.closed)


def buffered_text(path: str) -> TextIO:
    stream = open(path, "w", encoding="utf-8")  # noqa: SIM115
    stream.write("visible before the next operand\n")
    return stream


def read_text(path: str) -> str:
    with open(path, encoding="utf-8") as reader:
        return reader.read()


# Releasing a consumed temporary must flush before the RHS or body can read it.
print(buffered_text("truth-and.txt") and read_text("truth-and.txt"))
if buffered_text("truth-condition.txt"):
    print(read_text("truth-condition.txt"))
print(bool(buffered_text("truth-converted.txt")))
print(read_text("truth-converted.txt"))
selected = buffered_text("truth-retained.txt") or buffered_text("truth-skipped.txt")
print(selected.closed)
selected.write("still owned\n")
selected.close()
print(read_text("truth-retained.txt"))


with open("truth.txt", "w", encoding="utf-8") as writer:
    text_truth(writer)
    if writer:
        writer.write("present\n")
text_truth(writer)
text_truth(None)
with open("truth.bin", "wb") as binary:
    binary_truth(binary)
    if binary:
        binary.write(b"\0\xff")
binary_truth(binary)
binary_truth(None)
print(bool(ValueError("")))
print(not ValueError(""))
try:
    raise ValueError("")
except ValueError as error:
    if error:
        print("empty exception is true")
    print(bool(error))
