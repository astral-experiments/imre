from argparse import ArgumentParser


def read():
    parser = ArgumentParser(prog="options")
    options = parser
    options.add_argument("--key", default="name".upper())
    options.add_argument("--number", type=complex)
    options.add_argument("--letter", choices=list("ab"))
    options.add_argument("--a-b")
    options.add_argument("--a_b")
    return parser.parse_args()


print(read())
