import html
from html import entities


def current():
    markup = """<a title="'&amp;">"""
    print(html.escape(markup), html.escape(markup, quote=False))
    print(html.unescape("A &amp; B &copy C &NotEqualTilde; &unknown;"))
    print(html.unescape("&#65; &#x80; &#0; &#xD800; &#x110000;"))
    print(entities.html5["fjlig;"], entities.codepoint2name[38])
