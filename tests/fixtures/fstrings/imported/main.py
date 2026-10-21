"""Static imported calls and imported f-string bodies retain their bindings."""

from helpers import label

print(f"imported:{label(9)}")
