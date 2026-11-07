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
