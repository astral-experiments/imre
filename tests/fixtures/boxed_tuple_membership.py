def current():
    import gc

    def contains(needle, values):
        return needle in values

    assert contains("é", ("", "é", "e"))
    assert not contains("ab", ("a", "b"))
    assert contains("🦀", ("x", "🦀"))
    assert contains("\ud800", ("x", "\ud800"))
    assert "" not in () and "雪" in ("雪",)
    assert " " in (".", " ") and "é" not in ("e", 1)

    events = []

    class Answer:
        def __bool__(self):
            gc.collect()
            events.append("truth")
            return True

    class Match:
        def __eq__(self, other):
            events.append(other)
            return Answer()

    class Bomb:
        def __eq__(self, other):
            events.append("bomb")
            raise ValueError("compared")

    match = Match()
    bomb = Bomb()
    assert contains("present", ("absent", match, bomb))
    assert events == ["present", "truth"]
    assert contains("early", ("early", bomb)) and events == ["present", "truth"]
    events.clear()
    try:
        contains("late", ("early", bomb, "late"))
    except ValueError as error:
        assert str(error) == "compared"
    else:
        raise AssertionError("a later string bypassed an earlier comparison")
    assert events == ["bomb"]
    assert contains(bomb, (bomb,)) and events == ["bomb"]

    class Text(str):
        def __eq__(self, other):
            events.append("text")
            return True

    events.clear()
    assert contains(Text("absent"), ("x", "y"))
    assert contains("absent", (Text("x"), "y"))
    assert events == ["text", "text"]
    events.clear()
    assert match in ("left", "right") and events == ["left", "truth"]
    assert bomb not in ()
    assert contains("x", ["x"]) and contains("x", iter(("x",)))

    def part(value):
        events.append(value)
        return value

    events.clear()
    assert part("a") in (part("a"), part("b"))
    assert events == ["a", "a", "b"]
    assert 1 in (1, 2) == (1, 2)
    assert 0 not in (*[1, 2], 3)
    assert 10 not in (1, 2, 3, 4, 5, 6, 7, 8, 9)
