from _colorize import decolor, get_theme


def current():
    plain = get_theme(force_no_color=True).syntax
    assert plain.string == plain.reset == ""
    colored = get_theme(force_color=True).syntax
    assert colored.string == "\x1b[32m"
    assert decolor(colored.string + "hello" + colored.reset) == "hello"
