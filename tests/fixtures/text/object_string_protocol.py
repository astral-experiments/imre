class Printed:
    def __init__(self, text):
        self.text = text

    def __str__(self):
        print("in str")
        return self.text

    def __repr__(self):
        return "only repr"

print(str(Printed("π")))
