from io import BytesIO, StringIO
import token
import tokenize


def current():
    source = BytesIO(b"value += 1  # note\n")
    tokens = list(tokenize.tokenize(source.readline))
    operator, comment = [t for t in tokens if t.type in (token.OP, token.COMMENT)]
    assert (operator.string, operator.exact_type) == ("+=", token.PLUSEQUAL)
    assert (comment.string, comment.start) == ("# note", (1, 12))

    source = br"f'\N{EXCLAMATION MARK}'" + b"\n"
    tokens = list(tokenize.tokenize(BytesIO(source).readline))
    assert tokenize.untokenize(tokens) == source

    # A leading explicit join uses eight columns for both indentation comparisons.
    first = "if True:\n\t\\\n   pass\n"
    records = list(tokenize.generate_tokens(StringIO(first + "        pass\n").readline))
    assert [t.string for t in records if t.type == token.INDENT] == ["   "]
    try:
        list(tokenize.generate_tokens(StringIO(first + "\tpass\n").readline))
    except TabError as error:
        assert error.lineno == 4
    else:
        raise AssertionError("inconsistent indentation was accepted")

    text = "\ufeffvalue = 1\n"
    tokens = list(tokenize.generate_tokens(StringIO(text).readline))
    assert (tokens[0].type, tokens[0].string, tokens[0].end) == (token.NAME, "\ufeffvalue", (1, 6))
    assert tokenize.untokenize(tokens) == text

    invalid = BytesIO(b"# coding: no-such-encoding\n")
    try:
        tokenize.detect_encoding(invalid.readline)
    except SyntaxError as error:
        assert str(error) == "unknown encoding: no-such-encoding"
    else:
        raise AssertionError("invalid encoding cookie was accepted")

    cookie = b"# coding: iso-8859-15\n"
    source = BytesIO(cookie + b"\xa4\n")
    encoding, consumed = tokenize.detect_encoding(source.readline)
    assert (encoding, consumed) == ("iso-8859-15", [cookie])
    assert source.readline().decode(encoding) == "€\n"
    assert "€".encode(encoding) == b"\xa4"
