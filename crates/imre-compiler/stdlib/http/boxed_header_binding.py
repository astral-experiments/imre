# Capture the field parser before the transport defines its own HTTPMessage.
_parse_http_fields = HTTPMessage._parse
