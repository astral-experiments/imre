# From CPython 3.14.5 Lib/email/__init__.py.
def message_from_string(s, *args, **kws):
    """Parse a string into a Message object model.

    Optional _class and strict are passed to the Parser constructor.
    """
    from email.parser import Parser
    return Parser(*args, **kws).parsestr(s)

def message_from_bytes(s, *args, **kws):
    """Parse a bytes string into a Message object model.

    Optional _class and strict are passed to the Parser constructor.
    """
    from email.parser import BytesParser
    return BytesParser(*args, **kws).parsebytes(s)

def message_from_binary_file(fp, *args, **kws):
    """Read a binary file and parse its contents into a Message object model.

    Optional _class and strict are passed to the Parser constructor.
    """
    from email.parser import BytesParser
    return BytesParser(*args, **kws).parse(fp)
