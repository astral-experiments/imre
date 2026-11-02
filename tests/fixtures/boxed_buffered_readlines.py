def current():
    with open("lines.bin", "wb") as f:
        f.write(b"a\nbb\nlast")
    with open("lines.bin", "rb") as f:
        print(f.readlines(2), f.readlines(None), f.readlines())
        f.seek(0)
        print(f.readlines(1), f.readline(), f.readlines(-1))
    try:
        f.readlines()
    except ValueError as exc:
        print(type(exc).__name__)
