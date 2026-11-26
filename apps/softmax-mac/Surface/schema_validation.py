"""Required JSON Schema UTF-8 vocabulary: never ignore byte constraints.

Use validator(rule), not plain Draft202012Validator, for these product schemas.
Unknown mandatory vocabularies and malformed byte limits are configuration errors.
"""
import json
from pathlib import Path
from jsonschema import Draft202012Validator, ValidationError, validators

VOCAB = 'https://softmax.com/schemas/vocab/utf8-byte-length/v1'

def byte_bound(minimum):
    def check(validator, bound, value, schema):
        if type(bound) is not int or bound < 0:
            raise ValueError('invalid UTF-8 bound')
        if isinstance(value, str):
            try:
                length = len(value.encode('utf-8'))
            except UnicodeEncodeError:
                yield ValidationError('unpaired surrogate is not profile text')
                return
            if (length < bound if minimum else length > bound):
                yield ValidationError('UTF-8 byte length outside bound')
    return check

ProductValidator = validators.extend(Draft202012Validator, {
    'minUtf8Bytes': byte_bound(True), 'maxUtf8Bytes': byte_bound(False)
})

def validator(rule):
    schema = json.loads((Path(__file__).parent / 'softmax-app.schema.json').read_text())
    vocab = schema.get('$vocabulary', {})
    known = {VOCAB, 'https://json-schema.org/draft/2020-12/vocab/core',
             'https://json-schema.org/draft/2020-12/vocab/applicator',
             'https://json-schema.org/draft/2020-12/vocab/validation'}
    if vocab.get(VOCAB) is not True or any(required and uri not in known for uri, required in vocab.items()):
        raise ValueError('unsupported mandatory vocabulary')
    if rule not in schema['$defs']: raise ValueError('unknown product rule')
    schema['$ref'] = '#/$defs/' + rule
    ProductValidator.check_schema(schema)
    return ProductValidator(schema)
