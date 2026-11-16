from decimal import Decimal

price = Decimal("19.985")
credit = Decimal("-4.225")
quantity = Decimal("3.5")
cent = Decimal("0.01")
charge = (price * quantity).quantize(cent)
refund = (credit * quantity).quantize(cent)
print(charge, refund, charge + refund)

try:
    price * 0.5
except TypeError:
    pass
else:
    raise AssertionError("Decimal multiplied by a float")
