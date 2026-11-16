class Printed:
    def __init__(self, text):
        self.text = text

    def __str__(self):
        print("in str")
        return self.text

    def __repr__(self):
        return "only repr"

item = Printed("π")
print(str(item))
print(f"{item!s}")
print(f"{item!r}")
