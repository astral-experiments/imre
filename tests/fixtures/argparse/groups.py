from argparse import ArgumentParser, RawDescriptionHelpFormatter


def read():
    parser = ArgumentParser(prog="grouped", formatter_class=RawDescriptionHelpFormatter)
    sources = parser.add_argument_group("sources", "Input for %(prog)s.\r\nChoose one.")
    notes = parser.add_argument_group(title="notes", description="A group without actions.")
    empty = parser.add_argument_group("hidden empty group")
    sources.add_argument("--first", help="first source")
    parser.add_argument("-v", action="store_true")
    unnamed = parser.add_argument_group()
    unnamed.add_argument("--alt", help="alternate source")
    sources.add_argument("second", help="second source")
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("check", help="check inputs")
    settings = sub.add_argument_group("settings", description="Settings for %(prog)s.")
    settings.add_argument("--long-setting", help="choose a value")
    return parser.parse_args()


args = read()
print(args.second)
print(repr(args.first))
print(args.v)
