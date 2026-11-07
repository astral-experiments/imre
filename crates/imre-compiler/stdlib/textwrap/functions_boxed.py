def wrap(text, width=70, **kwargs):
    """Wrap a single paragraph of text, returning a list of wrapped lines.

    Reformat the single paragraph in 'text' so it fits in lines of no
    more than 'width' columns, and return a list of wrapped lines.  By
    default, tabs in 'text' are expanded with string.expandtabs(), and
    all other whitespace characters (including newline) are converted to
    space.  See TextWrapper class for available keyword args to customize
    wrapping behaviour.
    """
    w = TextWrapper(width=width, **kwargs)
    return w.wrap(text)

def fill(text, width=70, **kwargs):
    """Fill a single paragraph of text, returning a new string.

    Reformat the single paragraph in 'text' to fit in lines of no more
    than 'width' columns, and return a new string containing the entire
    wrapped paragraph.  As with wrap(), tabs are expanded and other
    whitespace characters converted to space.  See TextWrapper class for
    available keyword args to customize wrapping behaviour.
    """
    w = TextWrapper(width=width, **kwargs)
    return w.fill(text)

def shorten(text, width, **kwargs):
    """Collapse and truncate the given text to fit in the given width.

    The text first has its whitespace collapsed.  If it then fits in
    the *width*, it is returned as is.  Otherwise, as many words
    as possible are joined and then the placeholder is appended::

        >>> textwrap.shorten("Hello  world!", width=12)
        'Hello world!'
        >>> textwrap.shorten("Hello  world!", width=11)
        'Hello [...]'
    """
    w = TextWrapper(width=width, max_lines=1, **kwargs)
    return w.fill(' '.join(text.strip().split()))


# -- Loosely related functionality -------------------------------------

def dedent(text):
    """Remove any common leading whitespace from every line in `text`.

    This can be used to make triple-quoted strings line up with the left
    edge of the display, while still presenting them in the source code
    in indented form.

    Note that tabs and spaces are both treated as whitespace, but they
    are not equal: the lines "  hello" and "\\thello" are
    considered to have no common leading whitespace.

    Entirely blank lines are normalized to a newline character.
    """
    try:
        lines = text.split('\n')
    except (AttributeError, TypeError):
        msg = f'expected str object, not {type(text).__qualname__!r}'
        raise TypeError(msg) from None

    # Get length of leading whitespace, inspired by ``os.path.commonprefix()``.
    non_blank_lines = [l for l in lines if l and not l.isspace()]
    l1 = min(non_blank_lines, default='')
    l2 = max(non_blank_lines, default='')
    margin = 0
    for margin, c in enumerate(l1):
        if c != l2[margin] or c not in ' \t':
            break

    return '\n'.join([l[margin:] if not l.isspace() else '' for l in lines])


def indent(text, prefix, predicate=None):
    """Adds 'prefix' to the beginning of selected lines in 'text'.

    If 'predicate' is provided, 'prefix' will only be added to the lines
    where 'predicate(line)' is True. If 'predicate' is not provided,
    it will default to adding 'prefix' to all non-empty lines that do not
    consist solely of whitespace characters.
    """
    prefixed_lines = []
    if predicate is None:
        # str.splitlines(keepends=True) doesn't produce the empty string,
        # so we need to use `str.isspace()` rather than a truth test.
        # Inlining the predicate leads to a ~30% performance improvement.
        for line in text.splitlines(True):
            if not line.isspace():
                prefixed_lines.append(prefix)
            prefixed_lines.append(line)
    else:
        for line in text.splitlines(True):
            if predicate(line):
                prefixed_lines.append(prefix)
            prefixed_lines.append(line)
    return ''.join(prefixed_lines)
