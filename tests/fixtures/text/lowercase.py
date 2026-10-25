"""Default lowercasing includes expansions and the context-sensitive final sigma."""
for word in ["", "CPython-3.14_", "already lower", "İ", "ẞ", "𐐧𐐧",
             "Σ", "ΟΣ", "ΟΣΑ", "Ο\u0301Σ", "ΟΣ\u0301Α", "ΟΣ\u0301.",
             "A'Σ", "AΣ'A", "AΣ\u0345.", "AΣ\u0345A", "\0Σ", "Σ\0"]:
    print(word.lower())
