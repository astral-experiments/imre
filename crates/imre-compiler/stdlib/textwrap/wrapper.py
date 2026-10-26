"""TextWrapper and wrapping helpers adapted from CPython 3.14.5 Lib/textwrap.py (PSF-2.0)."""

import re


def _space(c: str) -> bool:
    return c in " \t\n\v\f\r"


def _word(c: str) -> bool:
    if c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_0123456789":
        return True
    if c in "-.!?'\",()&":
        return False
    return re.search(r"\w", c) is not None


def _letter(c: str) -> bool:
    # CPython's wordsep_re defines "letter" as a word character other than a
    # decimal digit: underscores and nondecimal numeric characters qualify.
    if c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_":
        return True
    if c in "-.!?'\",()&0123456789":
        return False
    return _word(c) and re.search(r"\d", c) is None


def _punctuation(c: str) -> bool:
    return _word(c) or c in "!\"'&.,?"


def _dash_end(chars: list[str], i: int) -> int:
    # Two or more hyphens form their own chunk between punctuation and a word.
    if i == 0 or i + 1 >= len(chars) or chars[i] != "-" or chars[i + 1] != "-":
        return i
    if not _punctuation(chars[i - 1]):
        return i
    j = i + 2
    while j < len(chars) and chars[j] == "-":
        j += 1
    if j < len(chars) and _word(chars[j]):
        return j
    return i


def _hyphen(chars: list[str], i: int) -> bool:
    # A single hyphen can terminate a word only when the letters on both
    # sides match wordsep_re's two lookaround alternatives.
    if chars[i] != "-" or i < 2 or i + 2 >= len(chars):
        return False
    if not _letter(chars[i - 1]) or not _letter(chars[i + 1]):
        return False
    if not _letter(chars[i - 2]) and not (i > 2 and chars[i - 2] == "-" and _letter(chars[i - 3])):
        return False
    return _letter(chars[i + 2]) or (
        chars[i + 2] == "-" and i + 3 < len(chars) and _letter(chars[i + 3])
    )


def _chunk_text(text: str, break_hyphens: bool = True) -> list[str]:
    # Keep whitespace chunks: _wrap_chunks decides which to drop at line edges.
    parts: list[str] = []
    # Native strings are UTF-8; indexing each position would rescan the prefix.
    chars = [c for c in text]
    i = 0
    while i < len(chars):
        start = i
        if _space(chars[i]):
            i += 1
            while i < len(chars) and _space(chars[i]):
                i += 1
        else:
            end = _dash_end(chars, i) if break_hyphens else i
            if end > i:
                i = end
            else:
                i += 1
                while i < len(chars) and not _space(chars[i]):
                    if break_hyphens and _hyphen(chars, i):
                        i += 1
                        break
                    if break_hyphens and _dash_end(chars, i) > i:
                        break
                    i += 1
        parts.append("".join(chars[start:i]))
    return parts


def _sentence(text: str) -> bool:
    i = len(text)
    if i > 0 and text[i - 1] in "\"'":
        i -= 1
    return i >= 2 and text[i - 1] in ".!?" and text[i - 2] in "abcdefghijklmnopqrstuvwxyz"


class TextWrapper:
    width: int
    initial_indent: str
    subsequent_indent: str
    expand_tabs: bool | int
    replace_whitespace: bool | int
    fix_sentence_endings: bool | int
    break_long_words: bool | int
    drop_whitespace: bool | int
    break_on_hyphens: bool | int
    tabsize: int
    max_lines: int | None
    placeholder: str

    def __init__(
        self,
        width: int = 70,
        initial_indent: str = "",
        subsequent_indent: str = "",
        expand_tabs: bool | int = True,
        replace_whitespace: bool | int = True,
        fix_sentence_endings: bool | int = False,
        break_long_words: bool | int = True,
        drop_whitespace: bool | int = True,
        break_on_hyphens: bool | int = True,
        tabsize: int = 8,
        *,
        max_lines: int | None = None,
        placeholder: str = " [...]",
    ) -> None:
        self.width = width
        self.initial_indent = initial_indent
        self.subsequent_indent = subsequent_indent
        self.expand_tabs = expand_tabs
        self.replace_whitespace = replace_whitespace
        self.fix_sentence_endings = fix_sentence_endings
        self.break_long_words = break_long_words
        self.drop_whitespace = drop_whitespace
        self.break_on_hyphens = break_on_hyphens
        self.tabsize = tabsize
        self.max_lines = max_lines
        self.placeholder = placeholder

    def _split(self, text: str) -> list[str]:
        return _chunk_text(text, self.break_on_hyphens is True)

    def _munge_whitespace(self, text: str) -> str:
        if self.expand_tabs:
            text = text.expandtabs(self.tabsize)
        if self.replace_whitespace:
            values: list[str] = []
            for c in text:
                values.append(" " if _space(c) else c)
            return "".join(values)
        return text

    def _fix_sentence_endings(self, chunks: list[str]) -> None:
        i = 0
        while i < len(chunks) - 1:
            if chunks[i + 1] == " " and _sentence(chunks[i]):
                chunks[i + 1] = "  "
                i += 2
            else:
                i += 1

    def _handle_long_word(
        self, reversed_chunks: list[str], cur_line: list[str], cur_len: int, width: int
    ) -> None:
        if width < 1:
            space_left = 1
        else:
            space_left = width - cur_len

        # If we're allowed to break long words, then do so: put as much
        # of the next chunk onto the current line as will fit.
        if self.break_long_words and space_left > 0:
            end = space_left
            chunk = reversed_chunks[-1]
            if self.break_on_hyphens and len(chunk) > space_left:
                # break after last hyphen, but only if there are
                # non-hyphens before it
                hyphen = space_left - 1
                while hyphen >= 0 and chunk[hyphen] != "-":
                    hyphen -= 1
                if hyphen > 0 and chunk[:hyphen].strip("-"):
                    end = hyphen + 1
            cur_line.append(chunk[:end])
            reversed_chunks[-1] = chunk[end:]

        # Otherwise, we have to preserve the long word intact.  Only add
        # it to the current line if there's nothing already there --
        # that minimizes how much we violate the width constraint.
        elif not cur_line:
            cur_line.append(reversed_chunks.pop())

    def _wrap_chunks(self, chunks: list[str]) -> list[str]:
        lines: list[str] = []
        if self.width <= 0:
            raise ValueError(f"invalid width {self.width!r} (must be > 0)")
        max_lines = self.max_lines
        if max_lines is not None:
            if max_lines > 1:
                indent = self.subsequent_indent
            else:
                indent = self.initial_indent
            if len(indent) + len(self.placeholder.lstrip()) > self.width:
                raise ValueError("placeholder too large for max width")

        # Arrange in reverse order so items can be efficiently popped
        # from a stack of chucks.
        left = 0
        right = len(chunks) - 1
        while left < right:
            saved = chunks[left]
            chunks[left] = chunks[right]
            chunks[right] = saved
            left += 1
            right -= 1

        while chunks:

            # Start the list of chunks that will make up the current line.
            # cur_len is just the length of all the chunks in cur_line.
            cur_line: list[str] = []
            cur_len = 0

            # Figure out which static string will prefix this line.
            if lines:
                indent = self.subsequent_indent
            else:
                indent = self.initial_indent

            # Maximum width for this line.
            width = self.width - len(indent)

            # First chunk on line is whitespace -- drop it, unless this
            # is the very beginning of the text (ie. no lines started yet).
            if self.drop_whitespace and chunks[-1].strip() == "" and lines:
                chunks.pop()

            while chunks:
                l = len(chunks[-1])

                # Can at least squeeze this chunk onto the current line.
                if cur_len + l <= width:
                    cur_line.append(chunks.pop())
                    cur_len += l

                # Nope, this line is full.
                else:
                    break

            # The current line is full, and the next chunk is too big to
            # fit on *any* line (not just this one).
            if chunks and len(chunks[-1]) > width:
                self._handle_long_word(chunks, cur_line, cur_len, width)
                cur_len = 0
                for chunk in cur_line:
                    cur_len += len(chunk)

            # If the last chunk on this line is all whitespace, drop it.
            if self.drop_whitespace and cur_line and cur_line[-1].strip() == "":
                cur_len -= len(cur_line[-1])
                cur_line.pop()

            if cur_line:
                if (
                    max_lines is None
                    or len(lines) + 1 < max_lines
                    or (
                        not chunks
                        or self.drop_whitespace
                        and len(chunks) == 1
                        and not chunks[0].strip()
                    )
                    and cur_len <= width
                ):
                    # Convert current line back to a string and store it in
                    # list of all lines (return value).
                    lines.append(indent + "".join(cur_line))
                else:
                    while cur_line:
                        if cur_line[-1].strip() and cur_len + len(self.placeholder) <= width:
                            cur_line.append(self.placeholder)
                            lines.append(indent + "".join(cur_line))
                            break
                        cur_len -= len(cur_line[-1])
                        cur_line.pop()
                    else:
                        if lines:
                            prev_line = lines[-1].rstrip()
                            if len(prev_line) + len(self.placeholder) <= self.width:
                                lines[-1] = prev_line + self.placeholder
                                break
                        lines.append(indent + self.placeholder.lstrip())
                    break

        return lines

    def _split_chunks(self, text: str) -> list[str]:
        text = self._munge_whitespace(text)
        return self._split(text)

    def wrap(self, text: str) -> list[str]:
        chunks = self._split_chunks(text)
        if self.fix_sentence_endings:
            self._fix_sentence_endings(chunks)
        return self._wrap_chunks(chunks)

    def fill(self, text: str) -> str:
        return "\n".join(self.wrap(text))


def wrap(
    text: str,
    width: int = 70,
    *,
    initial_indent: str = "",
    subsequent_indent: str = "",
    expand_tabs: bool | int = True,
    replace_whitespace: bool | int = True,
    fix_sentence_endings: bool | int = False,
    break_long_words: bool | int = True,
    drop_whitespace: bool | int = True,
    break_on_hyphens: bool | int = True,
    tabsize: int = 8,
    max_lines: int | None = None,
    placeholder: str = " [...]",
) -> list[str]:
    w = TextWrapper(
        width=width,
        initial_indent=initial_indent,
        subsequent_indent=subsequent_indent,
        expand_tabs=expand_tabs,
        replace_whitespace=replace_whitespace,
        fix_sentence_endings=fix_sentence_endings,
        break_long_words=break_long_words,
        drop_whitespace=drop_whitespace,
        break_on_hyphens=break_on_hyphens,
        tabsize=tabsize,
        max_lines=max_lines,
        placeholder=placeholder,
    )
    return w.wrap(text)


def fill(
    text: str,
    width: int = 70,
    *,
    initial_indent: str = "",
    subsequent_indent: str = "",
    expand_tabs: bool | int = True,
    replace_whitespace: bool | int = True,
    fix_sentence_endings: bool | int = False,
    break_long_words: bool | int = True,
    drop_whitespace: bool | int = True,
    break_on_hyphens: bool | int = True,
    tabsize: int = 8,
    max_lines: int | None = None,
    placeholder: str = " [...]",
) -> str:
    w = TextWrapper(
        width=width,
        initial_indent=initial_indent,
        subsequent_indent=subsequent_indent,
        expand_tabs=expand_tabs,
        replace_whitespace=replace_whitespace,
        fix_sentence_endings=fix_sentence_endings,
        break_long_words=break_long_words,
        drop_whitespace=drop_whitespace,
        break_on_hyphens=break_on_hyphens,
        tabsize=tabsize,
        max_lines=max_lines,
        placeholder=placeholder,
    )
    return w.fill(text)


def shorten(
    text: str,
    width: int,
    *,
    initial_indent: str = "",
    subsequent_indent: str = "",
    expand_tabs: bool | int = True,
    replace_whitespace: bool | int = True,
    fix_sentence_endings: bool | int = False,
    break_long_words: bool | int = True,
    drop_whitespace: bool | int = True,
    break_on_hyphens: bool | int = True,
    tabsize: int = 8,
    placeholder: str = " [...]",
) -> str:
    w = TextWrapper(
        width=width,
        max_lines=1,
        initial_indent=initial_indent,
        subsequent_indent=subsequent_indent,
        expand_tabs=expand_tabs,
        replace_whitespace=replace_whitespace,
        fix_sentence_endings=fix_sentence_endings,
        break_long_words=break_long_words,
        drop_whitespace=drop_whitespace,
        break_on_hyphens=break_on_hyphens,
        tabsize=tabsize,
        placeholder=placeholder,
    )
    return w.fill(" ".join(text.strip().split()))
