"""Printing live Python frames in the boxed runtime."""


def _walk_stack(frame):
    while frame is not None:
        yield frame, frame.f_lineno
        frame = frame.f_back


def print_stack(f=None, limit=None, file=None):
    """Print the stack up to f, defaulting to the caller's frame."""
    import linecache
    import sys
    from collections import deque
    from itertools import groupby, islice

    if f is None:
        f = sys._getframe().f_back
    frames = _walk_stack(f)
    if limit is None:
        limit = getattr(sys, 'tracebacklimit', None)
        if limit is not None and limit < 0:
            limit = 0
    if limit is not None:
        frames = islice(frames, limit) if limit >= 0 else deque(frames, maxlen=-limit)

    entries = []
    filenames = set()
    for frame, lineno in frames:
        code = frame.f_code
        filename = code.co_filename
        entries.append((code, lineno, filename, code.co_name))
        filenames.add(filename)
        linecache.lazycache(filename, frame.f_globals)
    for filename in filenames:
        linecache.checkcache(filename)

    # Resolve every line before calling a stream that may modify the source.
    rendered = []
    for code, lineno, filename, name in entries:
        line = ''
        if lineno is not None:
            line = linecache.getline(filename, lineno).rstrip()
            if not line and filename.startswith('<'):
                line = linecache._getline_from_code(code, lineno)
        display = '<stdin>' if filename.startswith('<stdin-') and filename.endswith('>') else filename
        text = f'  File "{display}", line {lineno}, in {name}\n'
        if line.strip():
            text += f'    {line.strip()}\n'
        # CPython groups repeats only when the previous filename, line and name exist.
        key = (filename, lineno, name)
        if lineno is None or name is None:
            key = object()
        rendered.append((key, text))

    if file is None:
        file = sys.stderr
    for _, group in groupby(reversed(rendered), lambda row: row[0]):
        count = 0
        for _, text in group:
            count += 1
            if count <= 3:
                print(text, file=file, end='')
        if count > 3:
            n = count - 3
            print(f'  [Previous line repeated {n} more time{"s" if n > 1 else ""}]\n', file=file, end='')
