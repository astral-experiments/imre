from string import capwords


def current():
    print(capwords(" aBC\t DeF "), capwords("-hELLO--WORLD-", "-"))
    print(list(map(ord, "Az")), str.capitalize("42ABC"))

    source = iter(["2", "bad", "4"])
    try:
        list(map(int, source))
    except ValueError:
        print(next(source))

    values = [2, 3]
    events = []

    def double(value):
        events.append(value)
        if value == 2:
            values.append(4)
        return value * 2

    for item in map(double, values):
        print(item, list(events))
