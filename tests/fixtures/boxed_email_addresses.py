from email import _header_value_parser, errors
from email._header_value_parser import get_addr_spec, get_angle_addr
from email.utils import _has_surrogates


def current():
    print(get_addr_spec is _header_value_parser.get_addr_spec)
    print(_has_surrogates(s="ü"), _has_surrogates(chr(0xDC80)))
    for parse, value in (
        (get_addr_spec, "user@example.org SIZE=32"),
        (get_angle_addr, '<"a b"@example.org> BODY=8BITMIME'),
        (get_angle_addr, "<>"),
    ):
        address, rest = parse(value)
        print(address.addr_spec, rest)
    try:
        get_angle_addr("<@example.org>")
    except errors.HeaderParseError as error:
        print(type(error).__name__, str(error))
