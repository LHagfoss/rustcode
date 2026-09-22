import io
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from laya_sidecar import MAX_LINE_BYTES, _bounded_lines, _diagnostic_line


class GuardedInput:
    def __init__(self, payload: bytes):
        self._stream = io.BytesIO(payload)

    def readline(self, size=-1):
        if size != MAX_LINE_BYTES + 1:
            raise AssertionError("reader was not bounded")
        return self._stream.readline(size)


class LayaSidecarTests(unittest.TestCase):
    def test_oversized_physical_line_is_rejected_in_bounded_chunks(self):
        messages = list(_bounded_lines(GuardedInput(b"x" * (MAX_LINE_BYTES * 32) + b"\n")))

        self.assertEqual(messages, [None])

    def test_diagnostics_are_fixed_categories_without_exception_text(self):
        line = _diagnostic_line("model_error")
        sanitized = _diagnostic_line("third-party exception")

        self.assertEqual(line, "Laya sidecar error: model_error")
        self.assertEqual(sanitized, "Laya sidecar error: internal_error")
        self.assertNotIn("third-party exception", sanitized)


if __name__ == "__main__":
    unittest.main()
