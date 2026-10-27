from selectors import EVENT_READ, PollSelector

class File:
    def __init__(self, number):
        self.number = number
    def fileno(self):
        return self.number

def current():
    first = 0
    with PollSelector() as selector:
        file = File(first)
        key = selector.register(file, EVENT_READ)
        for number in (2**31, 2**63, 2**100, -(2**100)):
            file.number = number
            for lookup in range(3):
                try:
                    if lookup == 0:
                        print("found", selector.get_key(file) is key)
                    elif lookup == 1:
                        print("found", selector.modify(file, EVENT_READ) is key)
                    elif number > 0:
                        selector.unregister(file)
                except KeyError:
                    print("missing")
                except ValueError:
                    print("invalid")
                except OverflowError:
                    print("overflow")
            try:
                selector.register(File(number), EVENT_READ)
            except OverflowError as error:
                print(str(error))
            except ValueError as error:
                print(str(error))
        file.number = first
        print(selector.unregister(file) is key)
