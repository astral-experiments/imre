try:
    (3).missing(print("evaluated argument"))
except AttributeError as error:
    print(error)
