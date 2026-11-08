import unittest

from acerfan.curve import PROFILES, Controller, Curve, validate_points


class CurveTest(unittest.TestCase):
    def test_interpolation(self):
        c = Curve([(40, 0), (60, 50), (80, 100)])
        self.assertEqual(c.duty(30), 0)
        self.assertEqual(c.duty(50), 25)
        self.assertEqual(c.duty(70), 75)
        self.assertEqual(c.duty(95), 100)

    def test_profiles_valid(self):
        for pts in PROFILES.values():
            validate_points(pts)

    def test_validation(self):
        for bad in ([(40, 0)], [(60, 10), (50, 20)], [(40, 50), (60, 20)],
                    [(10, 0), (60, 20)], [(40, 0), (60, 120)]):
            with self.assertRaises(ValueError):
                validate_points(bad)


class ControllerTest(unittest.TestCase):
    def make(self):
        return Controller(Curve([(40, 0), (80, 100)]), hysteresis=4, ramp_down=5,
                          floor_pct=20, floor_temp=60)

    def test_rises_immediately(self):
        c = self.make()
        self.assertEqual(c.step(50), 25)
        self.assertEqual(c.step(70), 75)

    def test_hysteresis_holds(self):
        c = self.make()
        c.step(70)                       # 75%
        self.assertEqual(c.step(68), 75)  # only 2 °C cooler: hold

    def test_ramp_down_limited(self):
        c = self.make()
        c.step(70)                        # 75%
        self.assertEqual(c.step(50), 70)  # cooled 20 °C, target 25 but -5/tick
        self.assertEqual(c.step(50), 65)

    def test_floor(self):
        c = Controller(Curve([(40, 0), (90, 10)]), floor_pct=20, floor_temp=60)
        self.assertEqual(c.step(65), 20)
        self.assertLess(c.step(45), 20)


if __name__ == "__main__":
    unittest.main()
