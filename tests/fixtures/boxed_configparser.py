import configparser


def current():
    parser = configparser.ConfigParser(inline_comment_prefixes=(";",))
    parser.read_string(
        "[DEFAULT]\nbase = /tmp\n[logging]\n# ignored\n"
        "Log Level: INFO ; user comment\nfile = %(base)s/log\n"
    )
    assert parser.get("logging", "log level") == "INFO"
    assert parser.get("logging", "file") == "/tmp/log"
    try:
        parser.read_string("[other]\na=one\na=two\n")
    except configparser.DuplicateOptionError as error:
        assert error.option == "a" and error.lineno == 3
    else:
        raise AssertionError("accepted a duplicate option")
