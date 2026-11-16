import unittest
from unittest.mock import patch

import generate_module


class GenerateModuleTests(unittest.TestCase):
    def test_filemode_preserves_the_upstream_body(self):
        body = '''    perm  = []  # Keep the upstream spelling.
    perm.append("    perm = []")
    return "".join(perm)
'''
        with patch.object(generate_module, "SOURCE") as source:
            source.read_text.return_value = "def filemode(\n    mode,\n):\n" + body
            for pure in (False, True):
                with self.subTest(pure=pure):
                    generated = generate_module.generate(pure=pure)
                    namespace = {"_MODE_MAX": 65535}
                    exec(generated, namespace)
                    self.assertEqual(namespace["filemode"](0), "    perm = []")
                    self.assertIn(body, generated)


if __name__ == "__main__":
    unittest.main()
