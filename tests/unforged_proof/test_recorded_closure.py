from typing import Any
import json
import unittest
from recorded_closure import require_model_reply_closure


def archive() -> dict[str, Any]:
    request = dict(id='a', correlation_id='c')
    message = {'event': {'kind': 'llm.reply', 'correlation_id': 'c',
                         'payload': {'request_id': 'a', 'status': 'succeeded'}}}
    change = dict(path='/data/inbox.jsonl', before=None,
                  after=(json.dumps(message)+'\n').encode().hex())
    return dict(initial_llm_requests=[], trailing_boundary=None, records=[
        dict(status='committed', boundary=[], llm_requests=[request]),
        dict(status='committed', boundary=[change], llm_requests=[])])


class RecordedClosureTests(unittest.TestCase):
    def test_complete_recorded_model_reply_without_provider(self):
        self.assertEqual(require_model_reply_closure(archive()), dict(requests=1, replies=1))

    def test_missing_duplicate_and_inapplicable_replies_refuse(self):
        missing = archive()
        missing['records'].pop()
        duplicate = archive()
        duplicate['records'][1]['boundary'] *= 2
        wrong = archive()
        change = wrong['records'][1]['boundary'][0]
        message = json.loads(bytes.fromhex(change['after']))
        message['event']['payload']['request_id'] = 'other'
        change['after'] = (json.dumps(message)+'\n').encode().hex()
        for trace in (missing, duplicate, wrong):
            with self.subTest(trace=trace), self.assertRaises(ValueError):
                require_model_reply_closure(trace)

    def test_trailing_reply_and_initial_pending_are_checked(self):
        trace = archive()
        trace['trailing_boundary'] = trace['records'].pop()
        trace['initial_llm_requests'] = trace['records'].pop()['llm_requests']
        self.assertEqual(require_model_reply_closure(trace), dict(requests=1, replies=1))
        trace['trailing_boundary'] = None
        with self.assertRaisesRegex(ValueError, 'missing recorded model reply'):
            require_model_reply_closure(trace)

    def test_nonmatching_correlation_and_aborted_requests_refuse(self):
        for mutation in ('correlation', 'aborted'):
            trace = archive()
            if mutation == 'aborted':
                trace['records'][0]['status'] = 'aborted'
            else:
                trace['records'][0]['llm_requests'][0]['correlation_id'] = 'other'
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                require_model_reply_closure(trace)
