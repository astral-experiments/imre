from pathlib import Path


for turn in range(4):
    try:
        Path("missing").read_text(encoding="utf-8")
    except FileNotFoundError:
        print("missing read")
    try:
        Path("tree").read_text(encoding="utf-8")
    except IsADirectoryError:
        print("directory read")
    try:
        Path("input.txt/child").read_text(encoding="utf-8")
    except NotADirectoryError:
        print("parent is file")
    try:
        Path("invalid.txt").read_text(encoding="utf-8")
    except UnicodeDecodeError:
        print("decode failure")
    try:
        Path("missing/child").write_text("not created", encoding="utf-8")
    except FileNotFoundError:
        print("missing write parent")
    try:
        Path("tree").write_text("not written", encoding="utf-8")
    except IsADirectoryError:
        print("directory write")
    try:
        Path("bad\0path").read_text(encoding="utf-8")
    except ValueError:
        print("nul read")
    try:
        Path("bad\0path").write_text("not written", encoding="utf-8")
    except ValueError:
        print("nul write")
    try:
        Path("failing-output").write_text("buffered failure", encoding="utf-8")
    except OSError:
        print("close reports failure")
    try:
        for item in Path("missing").iterdir():
            print("unreachable missing")
    except FileNotFoundError:
        print("missing directory")
    try:
        for item in Path("input.txt").iterdir():
            print("unreachable file")
    except NotADirectoryError:
        print("not directory")
    try:
        for item in Path("bad\0path").iterdir():
            print("unreachable nul")
    except ValueError:
        print("nul directory")
    print(len(list(Path("missing").glob("*.json"))))
    print(len(list(Path("input.txt").glob("*.json"))))
    print(len(list(Path("bad\0path").glob("literal"))))
    try:
        print(len(list(Path("bad\0path").glob("*"))))
    except ValueError:
        print("nul wildcard scan")
    print(len(list(zip([], Path("bad\0path").glob("*")))))

print(Path("sentinel.txt").read_text(encoding="utf-8"))
