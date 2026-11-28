"""Reuse stage-1 prefix semantics for input-only hosted native steps."""
from run import prefix

INPUT_FIELDS = ('trace_id','coglet','step','logical_time','seed','previous','engine','budgets','grants','events','boundary')

def step_payload(trace, index):
    records = trace['records']
    if type(index) is not int or not 0 <= index < len(records):
        raise ValueError('step index outside recorded archive')
    record = records[index]
    return {'prefix': prefix(trace, index),
            'input': {key: record[key] for key in INPUT_FIELDS}}
