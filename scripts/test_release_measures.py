import copy
import unittest
from evaluate_release_measures import TARGETS, evaluate


class ReleaseMeasures(unittest.TestCase):
    def report(self):
        measures = {}
        for name, (minimum, _, ceiling) in TARGETS.items():
            measures[name] = {'attempts': minimum, 'successes': minimum}
            if ceiling:
                measures[name]['latencies_ms'] = [ceiling] * minimum
        measures['task_completion']['unauthorised_actions'] = 0
        measures['false_wake'] = {'activations': 2, 'hours': 2}
        return dict(revision='a' * 40, platform='macos', hardware='test',
                    providers='test', speech='human', measures=measures)

    def test_complete_evidence_passes_and_boundary_percentile_is_nearest_rank(self):
        r = self.report()
        r['measures']['first_reply']['latencies_ms'][-1] = 99999
        self.assertEqual(evaluate(r), [])  # 19th of 20 is still at the ceiling.
        r['measures']['first_reply']['latencies_ms'][-2] = 99999
        self.assertTrue(evaluate(r))

    def test_missing_invalid_and_synthetic_evidence_fail_closed(self):
        base = self.report()
        for field, value in [('speech', 'generated'), ('revision', 'main'),
                             ('platform', 'all'), ('measures', [])]:
            r = copy.deepcopy(base); r[field] = value
            self.assertTrue(evaluate(r), field)
        for bad in [None, [], {}, {'measures': None}]:
            self.assertTrue(evaluate(bad))
        for value in [float('nan'), float('inf'), -1, True, '100']:
            r = copy.deepcopy(base)
            r['measures']['first_reply']['latencies_ms'][0] = value
            self.assertTrue(evaluate(r))

    def test_failures_cannot_disappear_from_success_denominator(self):
        r = self.report()
        r['measures']['wake'] = dict(attempts=40, successes=37)
        self.assertTrue(evaluate(r))
        r = self.report(); r['measures']['task_completion']['unauthorised_actions'] = 1
        self.assertTrue(evaluate(r))
        r = self.report(); r['measures']['cancellation']['successes'] = 9
        self.assertTrue(evaluate(r))
