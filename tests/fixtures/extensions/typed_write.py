import retainer


def identity(value: int) -> int:
    return value


def main() -> None:
    items = [42]
    retainer.remember(items, identity)
    try:
        retainer.store_bytes(b"foreign write")
    except TypeError:
        print(f"rejected {items[0]}")
    else:
        print(f"stored {items[0]!r}")
    retainer.release()
    try:
        retainer.wrong_return(b"bytes")
    except TypeError:
        print("rejected result")


if __name__ == "__main__":
    main()
