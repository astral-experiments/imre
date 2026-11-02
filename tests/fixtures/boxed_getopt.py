import getopt


def current():
    # The grammar used by test.support.smtpd.parseargs.
    shortopts = "nVhc:s:du"
    longopts = ["class=", "nosetuid", "version", "help", "size=", "debug", "smtputf8"]
    argv = ["-nu", "--class=DebuggingServer", "-s", "2048", "--", "localhost:8025"]
    opts, args = getopt.getopt(argv, shortopts, longopts)
    assert opts == [("-n", ""), ("-u", ""), ("--class", "DebuggingServer"), ("-s", "2048")]
    assert args == ["localhost:8025"]

    try:
        getopt.getopt(["--unrecognized"], shortopts, longopts)
    except getopt.GetoptError as error:
        assert getopt.error is type(error)
        assert error.opt == "unrecognized"
        assert str(error) == "option --unrecognized not recognized"
    else:
        raise AssertionError("accepted unknown long option")
