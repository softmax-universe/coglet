"""Synthetic gate negative controls, NOT acoustic measurements."""
import copy
import unittest
import analyze


def fixture():
    runs = []
    for i in range(20):
        runs.extend([
            dict(kind='barge', conformant=True, barge_to_acoustic_stop_ms=110,
                 play_to_acoustic_onset_ms=40, ptt_press_call_ms=1,
                 played_minus_acoustic_ms=-70, word_exact=True, over_credited_words=0,
                 tone_present_at_barge=True),
            dict(kind='capture', conformant=True, ptt_press_to_first_capture_ms=5,
                 ptt_release_to_end_event_ms=1),
            dict(kind='stimulus', stimulus_scheduled_to_capture_ms=20),
            dict(kind='duck', conformant=True, barge_to_acoustic_duck_ms=40,
                 duck_level_ratio=0.25, duck_render_continued=True,
                 duck_late_level_ratio=0.25, duck_level_snr_db=20, duck_late_snr_db=20,
                 tone_present_at_barge=True),
        ])
    return runs


class GateTests(unittest.TestCase):
    def setUp(self):
        self.runs = fixture()
        self.thresholds = analyze.derive(self.runs)

    def test_valid_synthetic_controls(self):
        self.assertEqual(analyze.check(self.thresholds, self.runs)[0], [])

    def test_missing_duck_is_not_pass(self):
        self.assertTrue(analyze.check(self.thresholds, [r for r in self.runs if r['kind'] != 'duck'])[0])

    def test_derive_refuses_no_duck(self):
        with self.assertRaises(ValueError):
            analyze.derive([r for r in self.runs if r['kind'] != 'duck'])

    def test_silence_or_unchanged_gain_is_not_duck(self):
        for ratio in (0, 1):
            runs = copy.deepcopy(self.runs)
            for r in runs:
                if r['kind'] == 'duck':
                    r['duck_level_ratio'] = ratio
            self.assertTrue(analyze.check(self.thresholds, runs)[0])

    def test_reduced_level_without_continuation_is_not_duck(self):
        for key, value in [('duck_render_continued', False), ('tone_present_at_barge', False),
                           ('conformant', False)]:
            runs = copy.deepcopy(self.runs)
            next(r for r in runs if r['kind'] == 'duck')[key] = value
            self.assertTrue(analyze.check(self.thresholds, runs)[0], key)

    def test_missing_latency_or_ratio_refuses(self):
        for key in ('barge_to_acoustic_duck_ms', 'duck_level_ratio'):
            runs = copy.deepcopy(self.runs)
            for r in runs:
                r.pop(key, None)
            self.assertTrue(analyze.check(self.thresholds, runs)[0])

    def test_noise_only_and_late_silence_or_unduck_refuse(self):
        for key, value in [('duck_level_snr_db', 0), ('duck_late_snr_db', 0),
                           ('duck_late_level_ratio', 0), ('duck_late_level_ratio', 1),
                           ('duck_level_ratio', float('nan'))]:
            runs = copy.deepcopy(self.runs)
            next(r for r in runs if r['kind'] == 'duck')[key] = value
            self.assertTrue(analyze.check(self.thresholds, runs)[0], key)

    def test_v2_requires_every_measurement(self):
        for kind, key in [('barge', 'barge_to_acoustic_stop_ms'),
                          ('barge', 'played_minus_acoustic_ms'), ('barge', 'word_exact'),
                          ('capture', 'ptt_press_to_first_capture_ms'),
                          ('stimulus', 'stimulus_scheduled_to_capture_ms')]:
            runs = copy.deepcopy(self.runs)
            next(r for r in runs if r['kind'] == kind).pop(key)
            self.assertTrue(analyze.check(self.thresholds, runs)[0], key)

    def test_v2_refuses_noisy_or_missing_tone(self):
        for key, value in [('noise_ok', False), ('tone_present_at_barge', False)]:
            runs = copy.deepcopy(self.runs)
            next(r for r in runs if r['kind'] == 'barge')[key] = value
            self.assertTrue(analyze.check(self.thresholds, runs)[0], key)
            with self.assertRaises(ValueError):
                analyze.derive(runs)

    def test_direct_flush_missing_or_inaudible_refuses(self):
        good = dict(kind='ringdown', frequency_hz=1400, snr_db=20,
                    flush_to_acoustic_stop_ms=100)
        self.assertEqual(analyze.check(self.thresholds, self.runs + [good])[0], [])
        for key, value in [('flush_to_acoustic_stop_ms', None), ('snr_db', 0)]:
            bad = dict(good, **{key: value})
            self.assertTrue(analyze.check(self.thresholds, self.runs + [bad])[0])

    def test_v2_cannot_omit_duck_gate(self):
        thresholds = copy.deepcopy(self.thresholds)
        thresholds.pop('duck')
        self.assertTrue(analyze.check(thresholds, self.runs)[0])
        thresholds = copy.deepcopy(self.thresholds)
        thresholds['latency_ms'].pop('barge_to_acoustic_duck_ms')
        self.assertTrue(analyze.check(thresholds, self.runs)[0])

    def test_historical_v1_thresholds_still_check_without_duck(self):
        old = copy.deepcopy(self.thresholds)
        old['version'] = 1
        old.pop('duck')
        old['latency_ms'].pop('barge_to_acoustic_duck_ms')
        self.assertEqual(analyze.check(old, [r for r in self.runs if r['kind'] != 'duck'])[0], [])


if __name__ == '__main__':
    unittest.main()
