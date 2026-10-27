from predicates import hard, soft

for key in ["regular", "class", "match", "type", "_", "Class", "λ"]:
    print(hard(key))
    print(soft(key))
