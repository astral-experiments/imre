with open("update.bin", "wb") as seed:
    seed.write(b"abcd")

with open("update.bin", "r+b", 0) as stream:
    assert stream.seek(1) == 1
    assert stream.write(b"XY") == 2
    assert stream.seek(0) == 0
    print(stream.mode, stream.read())
