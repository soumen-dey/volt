import unittest

from volt_engine import Engine, VoltError


class EngineTests(unittest.TestCase):
    def test_eval_persists_bindings(self):
        with Engine() as engine:
            self.assertEqual(engine.eval("let pythonBinding = 40; pythonBinding + 2"), "42")
            self.assertEqual(engine.eval("pythonBinding + 1"), "41")

    def test_callback_and_callback_error(self):
        with Engine() as engine:
            engine.register_number_function("doubleFromPython", lambda args: args[0] * 2)
            self.assertEqual(engine.eval("doubleFromPython(21)"), "42")
            with self.assertRaises(VoltError):
                engine.eval("doubleFromPython('21')")

    def test_budgets(self):
        with Engine(
            max_source_bytes=1024,
            max_execution_steps=100,
            max_call_depth=8,
            max_evaluation_time_ms=1000,
        ) as engine:
            self.assertEqual(engine.eval("40 + 2"), "42")
            with self.assertRaises(VoltError):
                engine.eval(" " * 1025)

    def test_embedded_nul_is_rejected_before_ffi(self):
        with Engine() as engine:
            with self.assertRaises(ValueError):
                engine.eval("1\0 + 2")
            with self.assertRaises(ValueError):
                engine.register_number_function("bad\0name", lambda args: 0)
            self.assertEqual(engine.eval("40 + 2"), "42")


if __name__ == "__main__":
    unittest.main()
