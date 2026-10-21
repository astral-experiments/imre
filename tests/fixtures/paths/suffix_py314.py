from pathlib import Path


for text, stem, suffix in [("file.", "file", "."), (".file.", ".file", "."), ("file..", "file.", "."), ("a.tar.", "a.tar", "."), ("...", "...", ""), ("..", "..", ""), ("..file", "..file", ""), ("..file.ext", "..file", ".ext")]:
    path = Path(text)
    assert path.stem == stem
    assert path.suffix == suffix
print("trailing-dot suffix passed")
