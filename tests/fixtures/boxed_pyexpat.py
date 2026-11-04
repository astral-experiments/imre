import pyexpat
from xml.parsers import expat
from xml.parsers.expat.errors import codes

assert expat.ParserCreate is pyexpat.ParserCreate
assert expat.errors is pyexpat.errors
assert expat.model is pyexpat.model
assert codes is pyexpat.errors.codes


def incremental():
    parser = expat.ParserCreate(None, "}")
    parser.ordered_attributes = True
    starts = []
    text = []
    parser.StartElementHandler = lambda tag, attrs: starts.append((tag, attrs))
    parser.CharacterDataHandler = text.append
    parser.Parse(b'<p:r xmlns:p="urn:p" b="2" p:a="&amp;">\xc3', False)
    parser.Parse(b'\xa9</p:r>', True)
    assert starts == [("urn:p}r", ["b", "2", "urn:p}a", "&"])]
    assert "".join(text) == "é"


def unknown_encodings():
    parser = expat.ParserCreate()
    parser.buffer_text = True
    events = []
    parser.StartElementHandler = lambda name, attrs: events.append((name, attrs))
    parser.CharacterDataHandler = events.append
    parser.Parse(b'<?xml version="1.0" encoding="iso8859"?><r city="Z\xfcrich">caf', False)
    assert events == [("r", {"city": "Zürich"}), "caf"]
    parser.Parse(b"\xe9</r>", True)
    assert events[-1] == "é"

    parser = expat.ParserCreate("cp1252")
    text = []
    parser.CharacterDataHandler = text.append
    parser.Parse(b"\n<r>\x80", False)
    assert text == ["€"]
    try:
        parser.Parse(b"\x81</r>", True)
    except expat.ExpatError as error:
        assert (error.code, error.lineno, error.offset) == (4, 2, 4)
    else:
        raise AssertionError("undefined cp1252 byte accepted")

    for encoding, kind, message in (
        ("x-NoSuchCodec", LookupError, "unknown encoding: x-NoSuchCodec"),
        ("shift_jis", ValueError, "multi-byte encodings are not supported"),
    ):
        parser = expat.ParserCreate(encoding)
        handler = text.append
        parser.CharacterDataHandler = handler
        try:
            parser.Parse(b"<r/>", True)
        except kind as error:
            assert str(error) == message
            assert parser.ErrorCode == 18
            assert parser.CharacterDataHandler is handler
        else:
            raise AssertionError(f"unexpectedly parsed {encoding}")


def callback_exception():
    parser = expat.ParserCreate()
    seen = []
    error = ValueError("stop")

    def first(name, attrs):
        seen.append(name)
        parser.StartElementHandler = second

    def second(name, attrs):
        seen.append(name)
        raise error

    parser.StartElementHandler = first
    try:
        parser.Parse("<a><b/><c/></a>", True)
    except ValueError as caught:
        assert caught is error
    else:
        raise AssertionError("callback exception was lost")
    assert seen == ["a", "b"]


def buffered_callback_exception():
    parser = expat.ParserCreate()
    parser.buffer_text = True
    error = ValueError("stop")

    def reject(text):
        raise error

    parser.CharacterDataHandler = reject
    try:
        parser.Parse("<r>bad", False)
    except ValueError as caught:
        assert caught is error
    else:
        raise AssertionError("buffered callback exception was lost")
    assert parser.CharacterDataHandler is None
    try:
        parser.Parse("</r>", True)
    except expat.ExpatError as caught:
        assert (caught.code, caught.lineno, caught.offset) == (36, 1, 6)
    else:
        raise AssertionError("continued parsing after callback failure")


def replace_buffered_handler(clear):
    parser = expat.ParserCreate()
    parser.buffer_text = True
    old_text, new_text, default_events = [], [], []
    parser.DefaultHandlerExpand = default_events.append
    new_handler = None if clear else new_text.append

    def old(text):
        old_text.append(text)
        if len(old_text) == 1:
            parser.CharacterDataHandler = new_handler
            parser.CharacterDataHandler = new_handler

    parser.CharacterDataHandler = old
    parser.Parse("<r>before", False)
    parser.Parse("after</r>", True)
    assert old_text == ["before", "before"]
    assert new_text == ([] if clear else ["after"])
    assert default_events == (["<r>", "after", "</r>"] if clear else ["<r>", "</r>"])


def resize_during_flush():
    parser = expat.ParserCreate()
    parser.buffer_size = 4
    events = []

    def resize(text):
        events.append(text)
        if len(events) == 1:
            parser.buffer_size = 10

    parser.CharacterDataHandler = resize
    parser.Parse("<r>abc<![CDATA[defgh]]>ij</r>", True)
    assert events == ["abc", "abc", "defghij"]


def buffered_text_after_syntax_error():
    for raises in (False, True):
        parser = expat.ParserCreate()
        parser.buffer_text = True
        events = []
        error = ValueError("old handler")

        def old_handler(text):
            events.append(text)
            if raises:
                raise error

        parser.CharacterDataHandler = old_handler
        try:
            parser.Parse("<r>good</x>", True)
        except expat.ExpatError as caught:
            assert (caught.code, caught.lineno, caught.offset) == (7, 1, 9)
        else:
            raise AssertionError("mismatched end tag accepted")
        assert events == []
        try:
            parser.CharacterDataHandler = events.append
        except ValueError as caught:
            assert raises and caught is error
            assert parser.CharacterDataHandler is None
        else:
            assert not raises
        assert events == ["good"]


def malformed():
    parser = expat.ParserCreate()
    parser.Parse(b"<r>\n", False)
    try:
        parser.Parse("<é>μ</r>".encode(), True)
    except expat.ExpatError as error:
        assert (error.code, error.lineno, error.offset) == (7, 2, 6)
        assert (parser.ErrorLineNumber, parser.ErrorColumnNumber) == (2, 6)
    else:
        raise AssertionError("mismatched end tag accepted")


incremental()
unknown_encodings()
callback_exception()
buffered_callback_exception()
replace_buffered_handler(False)
replace_buffered_handler(True)
resize_during_flush()
buffered_text_after_syntax_error()
malformed()
print("pyexpat ok")
