import argparse


def current():
    parser = argparse.ArgumentParser(prog="format", exit_on_error=False)
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--indent", type=int, default=4)
    group.add_argument("--tab", dest="indent", action="store_const", const="\t")
    parser.add_argument("infile", nargs="?", default="-")

    assert parser.parse_args([]).infile == "-"
    assert parser.parse_args(["--indent", "2", "input.json"]).indent == 2
    assert parser.parse_args(["--tab"]).indent == "\t"
    try:
        parser.parse_args(["--indent", "2", "--tab"])
    except argparse.ArgumentError as error:
        assert error.argument_name == "--tab"
    else:
        raise AssertionError("mutually exclusive options were accepted")
    print("parsed defaults, options and conflict")
