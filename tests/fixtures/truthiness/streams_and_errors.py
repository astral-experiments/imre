from typing import BinaryIO, TextIO


def text_truth(stream: TextIO | None) -> None:
    print(bool(stream))
    if stream:
        print(stream.closed)


def binary_truth(stream: BinaryIO | None) -> None:
    print(bool(stream))
    if stream:
        print(stream.closed)


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
