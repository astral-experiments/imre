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
    assert list(unified_diff(["old\n"], ["new\n"])) == [
        "--- \n", "+++ \n", "@@ -1 +1 @@\n", "-old\n", "+new\n"
    ]
