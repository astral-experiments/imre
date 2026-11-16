from textwrap import wrap

def format_lines(text: str, width: int, **options) -> list[str]:
    return wrap(text, width, **options)

print(format_lines("alpha beta gamma", 8, subsequent_indent=">"))
