

# http.server and http.client share the bounded header parser. Retain each
# physical line so Latin-1 header bytes such as 0x85 do not become line breaks.
HTTPMessage._parse = _parse_http_fields


def _parse_header_lines(header_lines, _class=HTTPMessage):
    headers = _class()
    _parse_http_fields(headers, [line.decode("iso-8859-1") for line in header_lines])
    return headers
