from unittest import TestCase


class ImportedCase(TestCase):
    def test_imported(self) -> None:
        """Imported class, retained through its local alias."""
        print("imported")
