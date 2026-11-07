import html.parser


class Parser(html.parser.HTMLParser):
    def handle_starttag(self, tag, attrs):
        print("start", tag, attrs)

    def handle_endtag(self, tag):
        print("end", tag)

    def handle_data(self, data):
        print("data", data)


parser = Parser()
parser.feed("<p title='> &am")
parser.feed("p;'>body</p" + " x=y" * 120_000 + ">")
parser.feed("<a a=" * 120_000)
parser.close()
