"""Inspect only the sealed unidiff attributes exposed by Imre."""

import sys
from pathlib import Path

from unidiff import Hunk, PatchedFile, PatchSet, UnidiffParseError


def inspect(hunk: Hunk) -> None:
    print(hunk.source_start)
    print(len(hunk))
    print(str(hunk))


def retained_files(patch: str) -> list[PatchedFile]:
    result: list[PatchedFile] = []
    patch_set = PatchSet(patch)
    modified = patch_set.modified_files
    if modified:
        modified.pop()
    print(len(patch_set.modified_files))
    print(len(patch_set))

    for file in patch_set:
        result.append(file)
    return result


def main() -> None:
    try:
        files = retained_files(Path(sys.argv[1]).read_bytes().decode("utf8"))
    except UnidiffParseError as error:
        print("UnidiffParseError")
        print(str(error))
        return

    for file in files:
        print(file.path)
        print(len(file))
        for hunk in file:
            inspect(hunk)


main()
