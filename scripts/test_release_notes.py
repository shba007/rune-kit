import unittest
from release_notes import extract


class ReleaseNotes(unittest.TestCase):
    def test_exact_section(self):
        self.assertEqual(extract("## 0.5.0 - today\n- Useful fix.\n\n## 0.4.0\n- Older.\n", "0.5.0"), "- Useful fix.\n")
    def test_reject_invalid_sections(self):
        for text in ["## 0.4.0\n- Older", "## 0.5.0\n", "## 0.5.0\n- TBD", "## 0.5.0\nA\n## 0.5.0\nB"]:
            with self.assertRaises(ValueError):
                extract(text, "0.5.0")


if __name__ == "__main__":
    unittest.main()
