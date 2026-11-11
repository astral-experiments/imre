from argparse import ArgumentParser as Parser


if __name__ == "__main__":
    parser = Parser(prog="guarded", epilog="Generate a rule.")
    parser.add_argument("--name", default="rule")
    print(parser.parse_args().name)
