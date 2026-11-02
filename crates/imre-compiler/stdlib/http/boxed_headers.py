

HTTPMessage._parse = _parse_http_fields


def _parse_header_lines(header_lines, _class=HTTPMessage):
    headers = _class()
    # MIME recognizes CR, LF and CRLF, preserving each ending in folded fields.
    text = b"".join(header_lines).decode("iso-8859-1")
    _parse_http_fields(headers, io.StringIO(text, newline="").readlines())
    return headers
