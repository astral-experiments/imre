# Directory and .pth processing from CPython 3.14.5 Lib/site.py. See licenses/CPython.txt.
import sys
import os
import _io as io
import stat


def _trace(message):
    if sys.flags.verbose:
        print(message, file=sys.stderr)


def makepath(*paths):
    dir = os.path.join(*paths)
    try:
        dir = os.path.abspath(dir)
    except OSError:
        pass
    return dir, os.path.normcase(dir)


def _init_pathinfo():
    """Return a set containing all existing file system items from sys.path."""
    d = set()
    for item in sys.path:
        try:
            if os.path.exists(item):
                _, itemcase = makepath(item)
                d.add(itemcase)
        except TypeError:
            continue
    return d


def addpackage(sitedir, name, known_paths):
    """Process a .pth file within the site-packages directory:
       For each line in the file, either combine it with sitedir to a path
       and add that to known_paths, or execute it if it starts with 'import '.
    """
    if known_paths is None:
        known_paths = _init_pathinfo()
        reset = True
    else:
        reset = False
    fullname = os.path.join(sitedir, name)
    try:
        st = os.lstat(fullname)
    except OSError:
        return
    if ((getattr(st, 'st_flags', 0) & stat.UF_HIDDEN) or
        (getattr(st, 'st_file_attributes', 0) & stat.FILE_ATTRIBUTE_HIDDEN)):
        _trace(f"Skipping hidden .pth file: {fullname!r}")
        return
    _trace(f"Processing .pth file: {fullname!r}")
    try:
        with io.open_code(fullname) as f:
            pth_content = f.read()
    except OSError:
        return

    try:
        # Accept BOM markers in .pth files as we do in source files
        # (Windows PowerShell 5.1 makes it hard to emit UTF-8 files without a BOM)
        pth_content = pth_content.decode("utf-8-sig")
    except UnicodeDecodeError:
        # Fallback to locale encoding for backward compatibility.
        # We will deprecate this fallback in the future.
        import locale
        pth_content = pth_content.decode(locale.getencoding())
        _trace(f"Cannot read {fullname!r} as UTF-8. "
               f"Using fallback encoding {locale.getencoding()!r}")

    for n, line in enumerate(pth_content.splitlines(), 1):
        if line.startswith("#"):
            continue
        if line.strip() == "":
            continue
        try:
            if line.startswith(("import ", "import\t")):
                exec(line)
                continue
            line = line.rstrip()
            dir, dircase = makepath(sitedir, line)
            if dircase not in known_paths and os.path.exists(dir):
                sys.path.append(dir)
                known_paths.add(dircase)
        except Exception as exc:
            print(f"Error processing line {n:d} of {fullname}:\n",
                  file=sys.stderr)
            import traceback
            for record in traceback.format_exception(exc):
                for line in record.splitlines():
                    print('  '+line, file=sys.stderr)
            print("\nRemainder of file ignored", file=sys.stderr)
            break
    if reset:
        known_paths = None
    return known_paths


def addsitedir(sitedir, known_paths=None):
    """Add 'sitedir' argument to sys.path if missing and handle .pth files in
    'sitedir'"""
    _trace(f"Adding directory: {sitedir!r}")
    if known_paths is None:
        known_paths = _init_pathinfo()
        reset = True
    else:
        reset = False
    sitedir, sitedircase = makepath(sitedir)
    if not sitedircase in known_paths:
        sys.path.append(sitedir)        # Add path component
        known_paths.add(sitedircase)
    try:
        names = os.listdir(sitedir)
    except OSError:
        return
    names = [name for name in names
             if name.endswith(".pth") and not name.startswith(".")]
    for name in sorted(names):
        addpackage(sitedir, name, known_paths)
    if reset:
        known_paths = None
    return known_paths
