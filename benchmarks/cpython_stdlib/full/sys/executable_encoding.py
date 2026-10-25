"""Exercise standalone text encoding, including names and error handling used by sys.executable."""


def convert(text: str, encoding: str, errors: str) -> None:
    try:
        print(repr(text.encode(encoding, errors)))
    except (LookupError, UnicodeEncodeError, ValueError) as exc:
        print(str(exc))


def marked(events: list[str], value: str) -> str:
    events.append(value)
    return value


def run() -> None:
    print('é☃'.encode())
    print('é'.encode(encoding='latin1'))
    print('é'.encode('utf_8_sig'))
    for encoding in ('ascii', 'iso8859-1', 'u8', 'missing-encoding'):
        for errors in ('strict', 'ignore', 'replace', 'backslashreplace', 'xmlcharrefreplace', 'missing-handler'):
            convert('¢☃𐌀!', encoding, errors)
    convert('fits', 'ascii', 'missing-handler')
    convert('fits', 'ascii', 'bad\0handler')
    convert('fits', 'bad\0encoding', 'strict')
    events: list[str] = []
    print(marked(events, '¢☃').encode(errors=marked(events, 'backslashreplace'), encoding=marked(events, 'us-ascii')))
    print(events)
    try:
        marked(events, 'receiver').encode(marked(events, 'ascii'), encoding=marked(events, 'duplicate'))
    except TypeError as exc:
        print(str(exc))
    print(events)
    try:
        'x'.encode(None)
    except TypeError as exc:
        print(str(exc))
    try:
        'x'.encode(errors=None)
    except TypeError as exc:
        print(str(exc))
    try:
        'x'.encode(encoding='ascii', errors='strict', extra=0)
    except TypeError as exc:
        print(str(exc))
    try:
        'x'.encode(extra=1)
    except TypeError as exc:
        print(str(exc))
    try:
        'x'.encode(marked(events, 'ascii'), marked(events, 'strict'), marked(events, 'excess'))
    except TypeError as exc:
        print(str(exc))
    print(events)


run()
