from helpers import File, copy, encode

with open("alias.bin", "wb") as output:
    stream: File = output
    stream.write(encode("é🌍"))
with open("alias.bin", "rb") as reader:
    print(copy(reader).decode())
