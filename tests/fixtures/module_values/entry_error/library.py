# Module ownership must close this buffered file during error cleanup.
# ruff: noqa: SIM115
FILE = open("entry.txt", "w", encoding="utf-8")
FILE.write("entry buffer")
