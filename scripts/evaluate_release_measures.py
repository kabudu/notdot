#!/usr/bin/env python3
"""Evaluate sanitised per-platform voice acceptance evidence, never fabricate it."""
import argparse
import json
import math
import re

TARGETS = {
    'wake': (40, .95, None),
    'wake_feedback': (20, .95, 1000),
    'first_reply': (20, .95, 6000),
    'cancellation': (10, 1, 500),
    'task_completion': (20, .95, None),
    'conversation_safety': (10, 1, None),
}


def count(value):
    return type(value) is int and 0 <= value <= 1000000


def evaluate(report):
    errors = []
    if not isinstance(report, dict):
        return ['Expected a report object']
    if not re.fullmatch(r'[0-9a-f]{40}', str(report.get('revision', ''))):
        errors.append('Record the exact 40-character Git revision')
    if report.get('platform') not in ('macos', 'windows', 'linux'):
        errors.append('Record macos, windows or linux')
    for key in ('hardware', 'providers'):
        if not isinstance(report.get(key), str) or not 1 <= len(report[key]) <= 200:
            errors.append(f'Record {key} (at most 200 characters)')
    if report.get('speech') != 'human':
        errors.append('Human speech acceptance is not established by generated clips')
    measures = report.get('measures', {})
    if not isinstance(measures, dict):
        return errors + ['Expected measures object']
    for name, (minimum, rate, ceiling) in TARGETS.items():
        value = measures.get(name, {})
        if not isinstance(value, dict):
            errors.append(f'{name}: expected counts'); continue
        attempts, successes = value.get('attempts'), value.get('successes')
        if not count(attempts) or not count(successes) or successes > attempts:
            errors.append(f'{name}: invalid attempt/success counts'); continue
        if attempts < minimum or successes < attempts * rate:
            errors.append(f'{name}: need at least {minimum} attempts and {rate:.0%} success')
        if ceiling is not None:
            samples = value.get('latencies_ms')
            if (not isinstance(samples, list) or not samples or len(samples) != successes
                    or any(type(v) not in (int, float) or not math.isfinite(v) or v < 0 for v in samples)):
                errors.append(f'{name}: require one finite latency per successful attempt')
            elif sorted(samples)[math.ceil(.95 * len(samples)) - 1] > ceiling:
                errors.append(f'{name}: p95 exceeds {ceiling} ms')
        if name == 'task_completion' and (not count(value.get('unauthorised_actions'))
                                         or value['unauthorised_actions'] != 0):
            errors.append('task_completion: no unauthorised actions allowed')
    false = measures.get('false_wake', {})
    if not isinstance(false, dict):
        errors.append('false_wake: expected counts')
    else:
        hours, activations = false.get('hours'), false.get('activations')
        if (type(hours) not in (int, float) or not math.isfinite(hours) or hours < 2
                or not count(activations) or activations > hours):
            errors.append('false_wake: need at least 2 hours and at most 1 activation per hour')
    return errors


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report')
    args = parser.parse_args()
    try:
        with open(args.report) as stream:
            raw = stream.read(1024 * 1024 + 1)
        if len(raw) > 1024 * 1024:
            raise ValueError('Report exceeds 1 MiB')
        errors = evaluate(json.loads(raw))
    except (OSError, ValueError) as error:
        errors = [str(error)]
    print(json.dumps({'accepted': not errors, 'issues': errors}, indent=2))
    raise SystemExit(bool(errors))
