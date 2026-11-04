def canonical(text: str, allowed: set[str]) -> str | None:
    folded = text.upper()
    if folded in allowed:
        return folded
    return None


def on_miss(text: str, allowed: set[str]) -> str:
    folded = text.lower()
    if folded in allowed:
        return "known"
    return folded


def lower_member(text: str, allowed: set[str]) -> bool:
    return text.lower() in allowed


def record_key(text: str, allowed: set[str], events: list[int]) -> str:
    events.append(1)
    allowed.add(text.upper())
    return text


def record_set(allowed: set[str], events: list[int]) -> set[str]:
    events.append(2)
    return allowed


def mutate_before_probe(text: str, allowed: set[str], events: list[int]) -> bool:
    return record_key(text, allowed, events).upper() in allowed


def ordered(text: str, allowed: set[str], events: list[int]) -> bool:
    return record_key(text, allowed, events).upper() in record_set(allowed, events)


def show(value: str | None) -> None:
    if value is None:
        print("<none>")
    else:
        print(value)


def main() -> None:
    allowed = {"AB", "SS", "FFI", "I", "i\u0307", "k", "σ", "ος", "å", "", "a" * 80}
    for text in ("", "ab", "absent", "ß", "ﬃ", "İ", "ı", "K", "Å", "Σ", "ΟΣ", "A" * 80, "x" * 89):
        show(canonical(text, allowed))
        print(on_miss(text, allowed))
        print(lower_member(text, allowed))
    events: list[int] = []
    print(mutate_before_probe("previously unseen length", allowed, events))
    print(events)
    print(ordered("four", allowed, events))
    print(events)
    allowed.discard("AB")
    show(canonical("ab", allowed))
    allowed.add("AB")
    show(canonical("ab", allowed))


main()
