import re


fcre = re.compile(r'^From ', re.MULTILINE)
NEWLINE_WITHOUT_FWSP = re.compile(r'\r\n[^ \t]|\r[^ \n\t]|\n[^ \t]')
NEWLINE_WITHOUT_FWSP_BYTES = re.compile(br'\r\n[^ \t]|\r[^ \n\t]|\n[^ \t]')


def unused() -> None:
    re.compile("marker").findall("marker")


print(bool(NEWLINE_WITHOUT_FWSP.search("\r\nX")))
print(fcre.sub('>From ', 'From first\ninline From untouched\nFrom next'))
print(bool(fcre.match('x\nFrom after')), bool(fcre.search('x\nFrom after')))
for header in (b'\r\nX', b'\n\xff', b'\r\n\t', b'clean'):
    print(bool(NEWLINE_WITHOUT_FWSP_BYTES.search(header)))


def bytes_subject() -> bytes:
    print('evaluate bytes subject')
    return b'From '


try:
    re.search('x', bytes_subject())  # ty: ignore[no-matching-overload]
except TypeError as error:
    print(str(error))

try:
    re.compile(fcre, re.MULTILINE)
except ValueError as error:
    print(str(error))
print('after recompile')
