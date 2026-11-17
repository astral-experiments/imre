from difflib import Match, SequenceMatcher, get_close_matches, ndiff, unified_diff


def current():
    matcher = SequenceMatcher(None, "health", "healt")
    longest = matcher.find_longest_match()
    print(type(longest) is Match, longest.a, longest.b, longest.size)
    print(matcher.get_matching_blocks(), matcher.ratio())
    print(matcher.get_opcodes())
    print(list(matcher.get_grouped_opcodes(1)))
    matcher.set_seq2("hello")
    print(matcher.get_matching_blocks(), matcher.ratio())
    print(SequenceMatcher(None, "", "").ratio())
    print(get_close_matches("healt", ["health", "hello", "visits"]))
    assert list(ndiff(["old\n"], ["new\n"])) == ["- old\n", "+ new\n"]
    assert list(ndiff(["alpha one\n"], ["alpha two\n"])) == [
        "- alpha one\n", "?        --\n", "+ alpha two\n", "?       ++\n",
    ]
    assert list(unified_diff(["old\n"], ["new\n"])) == [
        "--- \n", "+++ \n", "@@ -1 +1 @@\n", "-old\n", "+new\n"
    ]
    check_live_matcher_state()


def check_live_matcher_state():
    import gc

    matcher = SequenceMatcher(None, "ab", "yy")
    matcher.b2j["a"] = [0]
    matcher.b2j["b"] = [1]
    assert matcher.find_longest_match() == Match(0, 0, 2)
    assert matcher.ratio() == 1.0
    matcher = SequenceMatcher(None, "xa", "za")
    matcher.b2j["a"] = [1, 0]
    assert matcher.find_longest_match(0, 2, 0, 1) == Match(0, 0, 0)
    assert SequenceMatcher(None, "é雪", "x雪").find_longest_match() == Match(1, 1, 1)
    del matcher
    gc.collect()

    events = []

    def finalize(value):
        events.append((value.a, value.b, value.get_matching_blocks(), list(value.b2j.items())))

    SequenceMatcher.__del__ = finalize
    try:
        matcher = SequenceMatcher(None, "xab", "yab")
        assert matcher.ratio() == 2 / 3
        del matcher
        gc.collect()
        assert events == [("xab", "yab", [Match(1, 1, 2), Match(3, 3, 0)],
                           [("y", [0]), ("a", [1]), ("b", [2])])]
    finally:
        del SequenceMatcher.__del__
