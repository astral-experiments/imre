"""Live stack summaries from CPython 3.14.5 Lib/traceback.py."""

import collections
import itertools
import linecache
import sys
import textwrap
import _colorize
from contextlib import suppress

BUILTIN_EXCEPTION_LIMIT = object()


def _safe_string(value, what, func=str):
    try:
        return func(value)
    except:
        return f'<{what} {func.__name__}() failed>'


def print_list(extracted_list, file=None):
    """Print the list of tuples as returned by extract_tb() or
    extract_stack() as a formatted stack trace to the given file."""
    if file is None:
        file = sys.stderr
    for item in StackSummary.from_list(extracted_list).format():
        print(item, file=file, end="")


def format_list(extracted_list):
    """Format a list of tuples or FrameSummary objects for printing.

    Given a list of tuples or FrameSummary objects as returned by
    extract_tb() or extract_stack(), return a list of strings ready
    for printing.

    Each string in the resulting list corresponds to the item with the
    same index in the argument list.  Each string ends in a newline;
    the strings may contain internal newlines as well, for those items
    whose source text line is not None.
    """
    return StackSummary.from_list(extracted_list).format()


def format_stack(f=None, limit=None):
    """Shorthand for 'format_list(extract_stack(f, limit))'."""
    if f is None:
        f = sys._getframe().f_back
    return format_list(extract_stack(f, limit=limit))


def format_tb(tb, limit=None):
    """A shorthand for 'format_list(extract_tb(tb, limit))'."""
    return extract_tb(tb, limit=limit).format()


def extract_tb(tb, limit=None):
    """Return a StackSummary of the traceback, starting at its outermost frame."""
    return StackSummary._extract_from_extended_frame_gen(_walk_tb_positions(tb), limit=limit)


def _walk_tb_positions(tb):
    from traceback import _tb_position
    while tb is not None:
        frame = tb.tb_frame
        positions = _tb_position(tb)
        if positions is None:
            # Traceback-like objects can expose the Python instruction-position protocol.
            positions = (None, None, None, None)
            lasti = tb.tb_lasti
            if lasti >= 0:
                positions = next(itertools.islice(frame.f_code.co_positions(), lasti // 2, None))
            if positions[0] is None:
                positions = (tb.tb_lineno,) + positions[1:]
        yield frame, positions
        tb = tb.tb_next


def extract_stack(f=None, limit=None):
    """Extract the raw traceback from the current stack frame.

    The return value has the same format as for extract_tb().  The
    optional 'f' and 'limit' arguments have the same meaning as for
    print_stack().  Each item in the list is a quadruple (filename,
    line number, function name, text), and the entries are in order
    from oldest to newest stack frame.
    """
    if f is None:
        f = sys._getframe().f_back
    stack = StackSummary.extract(walk_stack(f), limit=limit)
    stack.reverse()
    return stack


class FrameSummary:
    """Information about a single frame from a traceback.

    - :attr:`filename` The filename for the frame.
    - :attr:`lineno` The line within filename for the frame that was
      active when the frame was captured.
    - :attr:`name` The name of the function or method that was executing
      when the frame was captured.
    - :attr:`line` The text from the linecache module for the
      of code that was running when the frame was captured.
    - :attr:`locals` Either None if locals were not supplied, or a dict
      mapping the name to the repr() of the variable.
    """

    __slots__ = ('filename', 'lineno', 'end_lineno', 'colno', 'end_colno',
                 'name', '_lines', '_lines_dedented', 'locals', '_code')

    def __init__(self, filename, lineno, name, *, lookup_line=True,
            locals=None, line=None,
            end_lineno=None, colno=None, end_colno=None, **kwargs):
        """Construct a FrameSummary.

        :param lookup_line: If True, `linecache` is consulted for the source
            code line. Otherwise, the line will be looked up when first needed.
        :param locals: If supplied the frame locals, which will be captured as
            object representations.
        :param line: If provided, use this instead of looking up the line in
            the linecache.
        """
        self.filename = filename
        self.lineno = lineno
        self.end_lineno = lineno if end_lineno is None else end_lineno
        self.colno = colno
        self.end_colno = end_colno
        self.name = name
        self._code = kwargs.get("_code")
        self._lines = line
        self._lines_dedented = None
        if lookup_line:
            self.line
        self.locals = {k: _safe_string(v, 'local', func=repr)
            for k, v in locals.items()} if locals else None

    def __eq__(self, other):
        if isinstance(other, FrameSummary):
            return (self.filename == other.filename and
                    self.lineno == other.lineno and
                    self.name == other.name and
                    self.locals == other.locals)
        if isinstance(other, tuple):
            return (self.filename, self.lineno, self.name, self.line) == other
        return NotImplemented

    def __getitem__(self, pos):
        return (self.filename, self.lineno, self.name, self.line)[pos]

    def __iter__(self):
        return iter([self.filename, self.lineno, self.name, self.line])

    def __repr__(self):
        return "<FrameSummary file {filename}, line {lineno} in {name}>".format(
            filename=self.filename, lineno=self.lineno, name=self.name)

    def __len__(self):
        return 4

    def _set_lines(self):
        if (
            self._lines is None
            and self.lineno is not None
            and self.end_lineno is not None
        ):
            lines = []
            for lineno in range(self.lineno, self.end_lineno + 1):
                # treat errors (empty string) and empty lines (newline) as the same
                line = linecache.getline(self.filename, lineno).rstrip()
                if not line and self._code is not None and self.filename.startswith("<"):
                    line = linecache._getline_from_code(self._code, lineno).rstrip()
                lines.append(line)
            self._lines = "\n".join(lines) + "\n"

    @property
    def _original_lines(self):
        # Returns the line as-is from the source, without modifying whitespace.
        self._set_lines()
        return self._lines

    @property
    def _dedented_lines(self):
        # Returns _original_lines, but dedented
        self._set_lines()
        if self._lines_dedented is None and self._lines is not None:
            self._lines_dedented = textwrap.dedent(self._lines)
        return self._lines_dedented

    @property
    def line(self):
        self._set_lines()
        if self._lines is None:
            return None
        # return only the first line, stripped
        return self._lines.partition("\n")[0].strip()


def walk_stack(f):
    """Walk a stack yielding the frame and line number for each frame.

    This will follow f.f_back from the given frame. If no frame is given, the
    current stack is used. Usually used with StackSummary.extract.
    """
    if f is None:
        f = sys._getframe().f_back

    def walk_stack_generator(frame):
        while frame is not None:
            yield frame, frame.f_lineno
            frame = frame.f_back

    return walk_stack_generator(f)


def walk_tb(tb):
    """Walk a traceback yielding the frame and line number for each frame.

    This will follow tb.tb_next (and thus is in the opposite order to
    walk_stack). Usually used with StackSummary.extract.
    """
    while tb is not None:
        yield tb.tb_frame, tb.tb_lineno
        tb = tb.tb_next


_RECURSIVE_CUTOFF = 3 # Also hardcoded in traceback.c.


class StackSummary(list):
    """A list of FrameSummary objects, representing a stack of frames."""

    @classmethod
    def extract(klass, frame_gen, *, limit=None, lookup_lines=True,
            capture_locals=False):
        """Create a StackSummary from a traceback or stack object.

        :param frame_gen: A generator that yields (frame, lineno) tuples
            whose summaries are to be included in the stack.
        :param limit: None to include all frames or the number of frames to
            include.
        :param lookup_lines: If True, lookup lines for each frame immediately,
            otherwise lookup is deferred until the frame is rendered.
        :param capture_locals: If True, the local variables from each frame will
            be captured as object representations into the FrameSummary.
        """
        def extended_frame_gen():
            for f, lineno in frame_gen:
                yield f, (lineno, None, None, None)

        return klass._extract_from_extended_frame_gen(
            extended_frame_gen(), limit=limit, lookup_lines=lookup_lines,
            capture_locals=capture_locals)

    @classmethod
    def _extract_from_extended_frame_gen(klass, frame_gen, *, limit=None,
            lookup_lines=True, capture_locals=False):
        # Same as extract but operates on a frame generator that yields
        # (frame, (lineno, end_lineno, colno, end_colno)) in the stack.
        # Only lineno is required, the remaining fields can be None if the
        # information is not available.
        builtin_limit = limit is BUILTIN_EXCEPTION_LIMIT
        if limit is None or builtin_limit:
            limit = getattr(sys, 'tracebacklimit', None)
            if limit is not None and limit < 0:
                limit = 0
        if limit is not None:
            if builtin_limit:
                frame_gen = tuple(frame_gen)
                frame_gen = frame_gen[len(frame_gen) - limit:]
            elif limit >= 0:
                frame_gen = itertools.islice(frame_gen, limit)
            else:
                frame_gen = collections.deque(frame_gen, maxlen=-limit)

        result = klass()
        fnames = set()
        for f, (lineno, end_lineno, colno, end_colno) in frame_gen:
            co = f.f_code
            filename = co.co_filename
            name = co.co_name
            fnames.add(filename)
            linecache.lazycache(filename, f.f_globals)
            # Must defer line lookups until we have called checkcache.
            f_locals = f.f_locals if capture_locals else None
            result.append(
                FrameSummary(filename, lineno, name,
                    lookup_line=False, locals=f_locals,
                    end_lineno=end_lineno, colno=colno, end_colno=end_colno,
                    _code=f.f_code,
                )
            )
        for filename in fnames:
            linecache.checkcache(filename)

        # If immediate lookup was desired, trigger lookups now.
        if lookup_lines:
            for f in result:
                f.line
        return result

    @classmethod
    def from_list(klass, a_list):
        """
        Create a StackSummary object from a supplied list of
        FrameSummary objects or old-style list of tuples.
        """
        # While doing a fast-path check for isinstance(a_list, StackSummary) is
        # appealing, idlelib.run.cleanup_traceback and other similar code may
        # break this by making arbitrary frames plain tuples, so we need to
        # check on a frame by frame basis.
        result = StackSummary()
        for frame in a_list:
            if isinstance(frame, FrameSummary):
                result.append(frame)
            else:
                filename, lineno, name, line = frame
                result.append(FrameSummary(filename, lineno, name, line=line))
        return result

    def format_frame_summary(self, frame_summary, **kwargs):
        """Format the lines for a single FrameSummary.

        Returns a string representing one frame involved in the stack. This
        gets called for every frame to be printed in the stack summary.
        """
        colorize = kwargs.get("colorize", False)
        row = []
        filename = frame_summary.filename
        if frame_summary.filename.startswith("<stdin-") and frame_summary.filename.endswith('>'):
            filename = "<stdin>"
        if colorize:
            theme = _colorize.get_theme(force_color=True).traceback
        else:
            theme = _colorize.get_theme(force_no_color=True).traceback
        row.append(
            '  File {}"{}"{}, line {}{}{}, in {}{}{}\n'.format(
                theme.filename,
                filename,
                theme.reset,
                theme.line_no,
                frame_summary.lineno,
                theme.reset,
                theme.frame,
                frame_summary.name,
                theme.reset,
            )
        )
        if frame_summary._dedented_lines and frame_summary._dedented_lines.strip():
            if (
                frame_summary.colno is None or
                frame_summary.end_colno is None
            ):
                # only output first line if column information is missing
                row.append(textwrap.indent(frame_summary.line, '    ') + "\n")
            else:
                # get first and last line
                all_lines_original = frame_summary._original_lines.splitlines()
                first_line = all_lines_original[0]
                # assume all_lines_original has enough lines (since we constructed it)
                last_line = all_lines_original[frame_summary.end_lineno - frame_summary.lineno]

                # character index of the start/end of the instruction
                start_offset = _byte_offset_to_character_offset(first_line, frame_summary.colno)
                end_offset = _byte_offset_to_character_offset(last_line, frame_summary.end_colno)

                all_lines = frame_summary._dedented_lines.splitlines()[
                    :frame_summary.end_lineno - frame_summary.lineno + 1
                ]

                # adjust start/end offset based on dedent
                dedent_characters = len(first_line) - len(all_lines[0])
                start_offset = max(0, start_offset - dedent_characters)
                end_offset = max(0, end_offset - dedent_characters)

                # When showing this on a terminal, some of the non-ASCII characters
                # might be rendered as double-width characters, so we need to take
                # that into account when calculating the length of the line.
                dp_start_offset = _display_width(all_lines[0], offset=start_offset)
                dp_end_offset = _display_width(all_lines[-1], offset=end_offset)

                # get exact code segment corresponding to the instruction
                segment = "\n".join(all_lines)
                segment = segment[start_offset:len(segment) - (len(all_lines[-1]) - end_offset)]

                # attempt to parse for anchors
                anchors = None
                show_carets = False
                with suppress(Exception):
                    anchors = _extract_caret_anchors_from_line_segment(segment)
                show_carets = self._should_show_carets(start_offset, end_offset, all_lines, anchors)

                result = []

                # only display first line, last line, and lines around anchor start/end
                significant_lines = {0, len(all_lines) - 1}

                anchors_left_end_offset = 0
                anchors_right_start_offset = 0
                primary_char = "^"
                secondary_char = "^"
                if anchors:
                    anchors_left_end_offset = anchors.left_end_offset
                    anchors_right_start_offset = anchors.right_start_offset
                    # computed anchor positions do not take start_offset into account,
                    # so account for it here
                    if anchors.left_end_lineno == 0:
                        anchors_left_end_offset += start_offset
                    if anchors.right_start_lineno == 0:
                        anchors_right_start_offset += start_offset

                    # account for display width
                    anchors_left_end_offset = _display_width(
                        all_lines[anchors.left_end_lineno], offset=anchors_left_end_offset
                    )
                    anchors_right_start_offset = _display_width(
                        all_lines[anchors.right_start_lineno], offset=anchors_right_start_offset
                    )

                    primary_char = anchors.primary_char
                    secondary_char = anchors.secondary_char
                    significant_lines.update(
                        range(anchors.left_end_lineno - 1, anchors.left_end_lineno + 2)
                    )
                    significant_lines.update(
                        range(anchors.right_start_lineno - 1, anchors.right_start_lineno + 2)
                    )

                # remove bad line numbers
                significant_lines.discard(-1)
                significant_lines.discard(len(all_lines))

                def output_line(lineno):
                    """output all_lines[lineno] along with carets"""
                    result.append(all_lines[lineno] + "\n")
                    if not show_carets:
                        return
                    num_spaces = len(all_lines[lineno]) - len(all_lines[lineno].lstrip())
                    carets = []
                    num_carets = dp_end_offset if lineno == len(all_lines) - 1 else _display_width(all_lines[lineno])
                    # compute caret character for each position
                    for col in range(num_carets):
                        if col < num_spaces or (lineno == 0 and col < dp_start_offset):
                            # before first non-ws char of the line, or before start of instruction
                            carets.append(' ')
                        elif anchors and (
                            lineno > anchors.left_end_lineno or
                            (lineno == anchors.left_end_lineno and col >= anchors_left_end_offset)
                        ) and (
                            lineno < anchors.right_start_lineno or
                            (lineno == anchors.right_start_lineno and col < anchors_right_start_offset)
                        ):
                            # within anchors
                            carets.append(secondary_char)
                        else:
                            carets.append(primary_char)
                    if colorize:
                        # Replace the previous line with a red version of it only in the parts covered
                        # by the carets.
                        line = result[-1]
                        colorized_line_parts = []
                        colorized_carets_parts = []

                        for color, group in itertools.groupby(itertools.zip_longest(line, carets, fillvalue=""), key=lambda x: x[1]):
                            caret_group = list(group)
                            if color == "^":
                                colorized_line_parts.append(theme.error_highlight + "".join(char for char, _ in caret_group) + theme.reset)
                                colorized_carets_parts.append(theme.error_highlight + "".join(caret for _, caret in caret_group) + theme.reset)
                            elif color == "~":
                                colorized_line_parts.append(theme.error_range + "".join(char for char, _ in caret_group) + theme.reset)
                                colorized_carets_parts.append(theme.error_range + "".join(caret for _, caret in caret_group) + theme.reset)
                            else:
                                colorized_line_parts.append("".join(char for char, _ in caret_group))
                                colorized_carets_parts.append("".join(caret for _, caret in caret_group))

                        colorized_line = "".join(colorized_line_parts)
                        colorized_carets = "".join(colorized_carets_parts)
                        result[-1] = colorized_line
                        result.append(colorized_carets + "\n")
                    else:
                        result.append("".join(carets) + "\n")

                # display significant lines
                sig_lines_list = sorted(significant_lines)
                for i, lineno in enumerate(sig_lines_list):
                    if i:
                        linediff = lineno - sig_lines_list[i - 1]
                        if linediff == 2:
                            # 1 line in between - just output it
                            output_line(lineno - 1)
                        elif linediff > 2:
                            # > 1 line in between - abbreviate
                            result.append(f"...<{linediff - 1} lines>...\n")
                    output_line(lineno)

                row.append(
                    textwrap.indent(textwrap.dedent("".join(result)), '    ', lambda line: True)
                )
        if frame_summary.locals:
            for name, value in sorted(frame_summary.locals.items()):
                row.append('    {name} = {value}\n'.format(name=name, value=value))

        return ''.join(row)

    def _should_show_carets(self, start_offset, end_offset, all_lines, anchors):
        with suppress(SyntaxError, ImportError):
            import ast
            tree = ast.parse('\n'.join(all_lines))
            if not tree.body:
                return False
            statement = tree.body[0]
            value = None
            def _spawns_full_line(value):
                return (
                    value.lineno == 1
                    and value.end_lineno == len(all_lines)
                    and value.col_offset == start_offset
                    and value.end_col_offset == end_offset
                )
            match statement:
                case ast.Return(value=ast.Call()):
                    if isinstance(statement.value.func, ast.Name):
                        value = statement.value
                case ast.Assign(value=ast.Call()):
                    if (
                        len(statement.targets) == 1 and
                        isinstance(statement.targets[0], ast.Name)
                    ):
                        value = statement.value
            if value is not None and _spawns_full_line(value):
                return False
        if anchors:
            return True
        if all_lines[0][:start_offset].lstrip() or all_lines[-1][end_offset:].rstrip():
            return True
        return False

    def format(self, **kwargs):
        """Format the stack ready for printing.

        Returns a list of strings ready for printing.  Each string in the
        resulting list corresponds to a single frame from the stack.
        Each string ends in a newline; the strings may contain internal
        newlines as well, for those items with source text lines.

        For long sequences of the same frame and line, the first few
        repetitions are shown, followed by a summary line stating the exact
        number of further repetitions.
        """
        colorize = kwargs.get("colorize", False)
        result = []
        last_file = None
        last_line = None
        last_name = None
        count = 0
        for frame_summary in self:
            formatted_frame = self.format_frame_summary(frame_summary, colorize=colorize)
            if formatted_frame is None:
                continue
            if (last_file is None or last_file != frame_summary.filename or
                last_line is None or last_line != frame_summary.lineno or
                last_name is None or last_name != frame_summary.name):
                if count > _RECURSIVE_CUTOFF:
                    count -= _RECURSIVE_CUTOFF
                    result.append(
                        f'  [Previous line repeated {count} more '
                        f'time{"s" if count > 1 else ""}]\n'
                    )
                last_file = frame_summary.filename
                last_line = frame_summary.lineno
                last_name = frame_summary.name
                count = 0
            count += 1
            if count > _RECURSIVE_CUTOFF:
                continue
            result.append(formatted_frame)

        if count > _RECURSIVE_CUTOFF:
            count -= _RECURSIVE_CUTOFF
            result.append(
                f'  [Previous line repeated {count} more '
                f'time{"s" if count > 1 else ""}]\n'
            )
        return result


def _byte_offset_to_character_offset(str, offset):
    as_utf8 = str.encode('utf-8')
    return len(as_utf8[:offset].decode("utf-8", errors="replace"))


_Anchors = collections.namedtuple(
    "_Anchors",
    [
        "left_end_lineno",
        "left_end_offset",
        "right_start_lineno",
        "right_start_offset",
        "primary_char",
        "secondary_char",
    ],
    defaults=["~", "^"]
)

def _extract_caret_anchors_from_line_segment(segment):
    """
    Given source code `segment` corresponding to a FrameSummary, determine:
        - for binary ops, the location of the binary op
        - for indexing and function calls, the location of the brackets.
    `segment` is expected to be a valid Python expression.
    """
    import ast

    try:
        # Without parentheses, `segment` is parsed as a statement.
        # Binary ops, subscripts, and calls are expressions, so
        # we can wrap them with parentheses to parse them as
        # (possibly multi-line) expressions.
        # e.g. if we try to highlight the addition in
        # x = (
        #     a +
        #     b
        # )
        # then we would ast.parse
        #     a +
        #     b
        # which is not a valid statement because of the newline.
        # Adding brackets makes it a valid expression.
        # (
        #     a +
        #     b
        # )
        # Line locations will be different than the original,
        # which is taken into account later on.
        tree = ast.parse(f"(\n{segment}\n)")
    except SyntaxError:
        return None

    if len(tree.body) != 1:
        return None

    lines = segment.splitlines()

    def normalize(lineno, offset):
        """Get character index given byte offset"""
        return _byte_offset_to_character_offset(lines[lineno], offset)

    def next_valid_char(lineno, col):
        """Gets the next valid character index in `lines`, if
        the current location is not valid. Handles empty lines.
        """
        while lineno < len(lines) and col >= len(lines[lineno]):
            col = 0
            lineno += 1
        assert lineno < len(lines) and col < len(lines[lineno])
        return lineno, col

    def increment(lineno, col):
        """Get the next valid character index in `lines`."""
        col += 1
        lineno, col = next_valid_char(lineno, col)
        return lineno, col

    def nextline(lineno, col):
        """Get the next valid character at least on the next line"""
        col = 0
        lineno += 1
        lineno, col = next_valid_char(lineno, col)
        return lineno, col

    def increment_until(lineno, col, stop):
        """Get the next valid non-"\\#" character that satisfies the `stop` predicate"""
        while True:
            ch = lines[lineno][col]
            if ch in "\\#":
                lineno, col = nextline(lineno, col)
            elif not stop(ch):
                lineno, col = increment(lineno, col)
            else:
                break
        return lineno, col

    def setup_positions(expr, force_valid=True):
        """Get the lineno/col position of the end of `expr`. If `force_valid` is True,
        forces the position to be a valid character (e.g. if the position is beyond the
        end of the line, move to the next line)
        """
        # -2 since end_lineno is 1-indexed and because we added an extra
        # bracket + newline to `segment` when calling ast.parse
        lineno = expr.end_lineno - 2
        col = normalize(lineno, expr.end_col_offset)
        return next_valid_char(lineno, col) if force_valid else (lineno, col)

    statement = tree.body[0]
    match statement:
        case ast.Expr(expr):
            match expr:
                case ast.BinOp():
                    # ast gives these locations for BinOp subexpressions
                    # ( left_expr ) + ( right_expr )
                    #   left^^^^^       right^^^^^
                    lineno, col = setup_positions(expr.left)

                    # First operator character is the first non-space/')' character
                    lineno, col = increment_until(lineno, col, lambda x: not x.isspace() and x != ')')

                    # binary op is 1 or 2 characters long, on the same line,
                    # before the right subexpression
                    right_col = col + 1
                    if (
                        right_col < len(lines[lineno])
                        and (
                            # operator char should not be in the right subexpression
                            expr.right.lineno - 2 > lineno or
                            right_col < normalize(expr.right.lineno - 2, expr.right.col_offset)
                        )
                        and not (ch := lines[lineno][right_col]).isspace()
                        and ch not in "\\#"
                    ):
                        right_col += 1

                    # right_col can be invalid since it is exclusive
                    return _Anchors(lineno, col, lineno, right_col)
                case ast.Subscript():
                    # ast gives these locations for value and slice subexpressions
                    # ( value_expr ) [ slice_expr ]
                    #   value^^^^^     slice^^^^^
                    # subscript^^^^^^^^^^^^^^^^^^^^

                    # find left bracket
                    left_lineno, left_col = setup_positions(expr.value)
                    left_lineno, left_col = increment_until(left_lineno, left_col, lambda x: x == '[')
                    # find right bracket (final character of expression)
                    right_lineno, right_col = setup_positions(expr, force_valid=False)
                    return _Anchors(left_lineno, left_col, right_lineno, right_col)
                case ast.Call():
                    # ast gives these locations for function call expressions
                    # ( func_expr ) (args, kwargs)
                    #   func^^^^^
                    # call^^^^^^^^^^^^^^^^^^^^^^^^

                    # find left bracket
                    left_lineno, left_col = setup_positions(expr.func)
                    left_lineno, left_col = increment_until(left_lineno, left_col, lambda x: x == '(')
                    # find right bracket (final character of expression)
                    right_lineno, right_col = setup_positions(expr, force_valid=False)
                    return _Anchors(left_lineno, left_col, right_lineno, right_col)

    return None

_WIDE_CHAR_SPECIFIERS = "WF"

def _display_width(line, offset=None):
    """Calculate the extra amount of width space the given source
    code segment might take if it were to be displayed on a fixed
    width output device. Supports wide unicode characters and emojis."""

    if offset is None:
        offset = len(line)

    # Fast track for ASCII-only strings
    if line.isascii():
        return offset

    import unicodedata

    return sum(
        2 if unicodedata.east_asian_width(char) in _WIDE_CHAR_SPECIFIERS else 1
        for char in line[:offset]
    )


def print_stack(f=None, limit=None, file=None):
    """Print a stack trace from its invocation point.

    The optional 'f' argument can be used to specify an alternate
    stack frame at which to start. The optional 'limit' and 'file'
    arguments have the same meaning as for print_exception().
    """
    if f is None:
        f = sys._getframe().f_back
    print_list(extract_stack(f, limit=limit), file=file)
