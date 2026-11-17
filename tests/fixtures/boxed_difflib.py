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
    check_gc_chain_source()
    check_constructor_dispatch()
    check_unicode_index_order_and_popularity()
    check_matching_blocks_dispatch()
    check_ratio_dispatch()
    check_position_integer_identity()


def check_live_matcher_state():
    import difflib
    import gc
    import sys

    matcher = SequenceMatcher(None, "ab", "yy")
    matcher.b2j["a"] = [0]
    matcher.b2j["b"] = [1]
    assert matcher.find_longest_match() == Match(0, 0, 2)
    assert matcher.ratio() == 1.0
    matcher = SequenceMatcher(None, "xa", "za")
    matcher.b2j["a"] = [1, 0]
    assert matcher.find_longest_match(0, 2, 0, 1) == Match(0, 0, 0)
    assert SequenceMatcher(None, "é雪", "x雪").find_longest_match() == Match(1, 1, 1)
    assert SequenceMatcher(None, "x", "y" * 4096, False).find_longest_match() == Match(0, 0, 0)
    assert SequenceMatcher(None, "Ω", "雪" * 4096, False).find_longest_match() == Match(0, 0, 0)
    del matcher
    gc.collect()

    def inspect_match(*parts):
        frame = sys._getframe(1)
        assert (frame.f_locals["besti"], frame.f_locals["bestj"], frame.f_locals["bestsize"]) == parts
        return Match(*parts)

    difflib.Match = inspect_match
    try:
        assert SequenceMatcher(None, "xab", "yab").find_longest_match() == Match(1, 1, 2)
    finally:
        difflib.Match = Match

    previous_new = Match.__dict__["__new__"]

    def inspect_new(cls, *parts):
        frame = sys._getframe(1)
        assert (frame.f_locals["besti"], frame.f_locals["bestj"], frame.f_locals["bestsize"]) == parts
        return tuple.__new__(cls, parts)

    Match.__new__ = staticmethod(inspect_new)
    try:
        assert tuple(SequenceMatcher(None, "xab", "yab").find_longest_match()) == (1, 1, 2)
    finally:
        Match.__new__ = previous_new

    constructor_globals = Match.__new__.__globals__
    previous_tuple_new = constructor_globals["_tuple_new"]

    def inspect_tuple_new(cls, parts):
        frame = sys._getframe(2)
        assert (frame.f_locals["besti"], frame.f_locals["bestj"], frame.f_locals["bestsize"]) == parts
        return tuple.__new__(cls, parts)

    constructor_globals["_tuple_new"] = inspect_tuple_new
    try:
        assert tuple(SequenceMatcher(None, "xab", "yab").find_longest_match()) == (1, 1, 2)
    finally:
        constructor_globals["_tuple_new"] = previous_tuple_new

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


def check_gc_chain_source():
    import gc
    import sys

    seen = []
    old = gc.get_threshold()

    def observe(phase, info):
        if phase != "start":
            return
        frame = sys._getframe(1)
        while frame is not None:
            if frame.f_code.co_name == "__chain_b":
                seen.append(True)
                break
            frame = frame.f_back

    gc.callbacks.append(observe)
    try:
        gc.set_threshold(1, 1, 1)
        matcher = SequenceMatcher(None, "", "abcdefghij" * 20, False)
    finally:
        gc.callbacks.remove(observe)
        gc.set_threshold(*old)
    assert seen and len(matcher.b2j) == 10


def check_constructor_dispatch():
    matcher = SequenceMatcher(None, "a", "b", False)
    assert list(matcher.__dict__) == [
        "isjunk", "a", "b", "autojunk", "matching_blocks", "opcodes",
        "fullbcount", "b2j", "bjunk", "bpopular",
    ]
    seen = []
    original = SequenceMatcher.set_seqs

    def observe(self, a, b):
        seen.append((self.isjunk, self.a, self.b, self.autojunk, a, b))

    SequenceMatcher.set_seqs = observe
    try:
        SequenceMatcher(None, "a", "b", False)
    finally:
        SequenceMatcher.set_seqs = original
    assert seen == [(None, None, None, False, "a", "b")]


def check_unicode_index_order_and_popularity():
    text = "雪" * 4 + "Ω" * 192 + "é🧪éx"
    matcher = SequenceMatcher(None, "", text)
    assert list(matcher.b2j.items()) == [("é", [196, 198]), ("🧪", [197]), ("x", [199])]
    assert matcher.bjunk == set()
    assert matcher.bpopular == {"雪", "Ω"}
    matcher = SequenceMatcher(None, "", text, autojunk=False)
    assert list(matcher.b2j) == ["雪", "Ω", "é", "🧪", "x"]
    assert matcher.b2j["雪"] == [0, 1, 2, 3]


def check_matching_blocks_dispatch():
    import sys

    matcher = SequenceMatcher(None, "abxcd", "abycd")
    state = matcher.__dict__
    blocks = matcher.get_matching_blocks()
    assert state["matching_blocks"] is blocks
    assert blocks == [Match(0, 0, 2), Match(3, 3, 2), Match(5, 5, 0)]
    blocks[0] = Match(0, 0, 1)
    assert matcher.get_matching_blocks() is blocks and matcher.ratio() == 0.6
    marker = object()
    matcher.matching_blocks = marker
    assert matcher.get_matching_blocks() is marker

    previous_make = Match.__dict__["_make"]
    seen = []

    def inspect_make(cls, triple):
        caller = sys._getframe(1)
        assert caller.f_code.co_name == "get_matching_blocks"
        seen.append(tuple(caller.f_locals["non_adjacent"]))
        return tuple.__new__(cls, triple)

    Match._make = classmethod(inspect_make)
    try:
        assert SequenceMatcher(None, "a", "a").get_matching_blocks() == [
            Match(0, 0, 1), Match(1, 1, 0)
        ]
    finally:
        Match._make = previous_make
    assert seen == [((0, 0, 1), (1, 1, 0))] * 2


def check_ratio_dispatch():
    import difflib
    import sys

    empty = SequenceMatcher(None, "", "")
    assert empty.ratio() is empty.ratio()
    matcher = SequenceMatcher(None, "ab", "ab")
    matcher.matching_blocks = [(0, 0, -1), [0, 0, 2]]
    assert matcher.ratio() == 0.5

    original = difflib._calculate_ratio

    def inspect(matches, length):
        caller = sys._getframe(1)
        assert caller.f_code.co_name == "ratio"
        assert caller.f_locals["matches"] == matches
        return matches, length

    difflib._calculate_ratio = inspect
    try:
        assert matcher.ratio() == (1, 4)
    finally:
        difflib._calculate_ratio = original


def check_position_integer_identity():
    import gc

    ascii_matcher = SequenceMatcher(None, "", "x" * 300, False)
    unicode_matcher = SequenceMatcher(None, "", "雪" * 300, False)
    ascii_positions = ascii_matcher.b2j["x"]
    unicode_positions = unicode_matcher.b2j["雪"]
    assert ascii_positions[0] is unicode_positions[0]
    assert ascii_positions[256] is unicode_positions[256]
    assert ascii_positions[257] is not unicode_positions[257]
    del ascii_matcher, unicode_matcher
    gc.collect()
    assert ascii_positions == unicode_positions == list(range(300))
