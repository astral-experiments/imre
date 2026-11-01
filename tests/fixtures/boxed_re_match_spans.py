import re


class Index:
    def __index__(self):
        return 1


def show(pattern, subject):
    match = re.compile(pattern).search(subject, 1)
    for group in [0, Index(), 'last', 3]:
        print(match.start(group), match.end(group), match.span(group))
    print(match.group(1, 'last'), match.start(), match.end(), match.span())
    for args in [(4,), (-1,), ('missing',)]:
        try:
            match.end(*args)
        except IndexError as error:
            print(error)
        else:
            raise AssertionError('invalid group accepted')


def current():
    show(r'(é)(?P<last>👋)(x)?$', 'äé👋\n')
    show(rb'(\xff)(?P<last>z)(x)?$', b'!\xffz\n')
