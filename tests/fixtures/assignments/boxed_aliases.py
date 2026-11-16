values = {1} ^ {2}

type Count = int
type Units = Count
units: Units = 3

# A scalar recovered from a circular alias is insufficient to check an initializer.
type Loop = Loop | int
label: Loop = "pending"
print(units, label)
