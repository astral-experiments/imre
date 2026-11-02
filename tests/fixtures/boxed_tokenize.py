from io import BytesIO
import token
import tokenize


def current():
    source = BytesIO(b"value += 1  # note\n")
    tokens = list(tokenize.tokenize(source.readline))
    operator, comment = [t for t in tokens if t.type in (token.OP, token.COMMENT)]
    assert (operator.string, operator.exact_type) == ("+=", token.PLUSEQUAL)
    assert (comment.string, comment.start) == ("# note", (1, 12))

    invalid = BytesIO(b"# coding: no-such-encoding\n")
    try:
        tokenize.detect_encoding(invalid.readline)
    except SyntaxError as error:
        assert str(error) == "unknown encoding: no-such-encoding"
    else:
        raise AssertionError("invalid encoding cookie was accepted")
