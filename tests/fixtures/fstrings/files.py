"""F-string writes preserve counts, receiver roots, and pre-write argument effects."""

from typing import TextIO


def close_argument(stream: TextIO) -> str:
    print("closing argument")
    stream.close()
    return "closed"


def fail() -> str:
    print("failing argument")
    raise ValueError("field failure")


def remove_owner(streams: list[TextIO]) -> str:
    streams.pop()
    return "receiver retained"


def main() -> None:
    with open("output.txt", "w", encoding="utf-8") as stream:
        stream.write(f"prefix:{17}:λ\0🙂\n")
        print(stream.write(f"{'λ🙂'}{None}\0"))
        print(stream.write(f"{''}"))
        try:
            stream.write(f"must not write:{close_argument(stream)}:tail")
        except ValueError:
            print("closed after fields")
    with open("late.txt", "w", encoding="utf-8") as stream:
        try:
            stream.write(f"must not write:{fail()}:tail")
        except ValueError:
            print("no partial expression write")
    with open("output.txt", "r", encoding="utf-8") as reader:
        try:
            reader.write(f"{''}")
        except OSError:
            print("empty interpolation checks write mode")
    try:
        reader.write(f"{''}")
    except ValueError:
        print("empty interpolation checks closed state")
    try:
        reader.write(f"{fail()}")
    except ValueError as problem:
        print(f"argument wins:{problem}")
    streams: list[TextIO] = [open("rooted.txt", "w", encoding="utf-8")]  # noqa: SIM115 — receiver loses its last external owner
    streams[0].write(
        f"{1000000000000000000000000000000000000000031}:{remove_owner(streams)}"
    )
    print(len(streams))


main()
