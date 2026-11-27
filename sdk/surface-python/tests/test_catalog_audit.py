"""Fail-closed catalog admission audit regressions."""
import pytest
from test_contract_profile import artifact, catalog
from coglet_surface.codec import validate


@pytest.mark.parametrize('pattern', [r'\W+', r'\D+', r'\S+', r'(?i)k', r'[[:alpha:]]+', r'(?=a)a', r'(a)\1'])
def test_nonportable_catalog_regex_rejected(pattern):
    value = artifact()
    value['rules']['extra'] = dict(op='regexp', pattern=pattern)
    with pytest.raises(ValueError):
        catalog(value)


@pytest.mark.parametrize('node', [dict(op=[], name='tstr'), dict(op='primitive', name=[]),
                                 dict(op='regexp', pattern='[')])
def test_malformed_ir_fails_with_value_error(node):
    value = artifact()
    value['rules']['extra'] = node
    with pytest.raises(ValueError):
        catalog(value)


def test_structured_literal_preserves_nested_boolean_integer_distinction():
    schema = {'value': dict(op='literal', value={'items': [True]})}
    validate('value', {'items': [True]}, schema)
    with pytest.raises(ValueError):
        validate('value', {'items': [1]}, schema)


@pytest.mark.parametrize('pattern', [r'^[a-z]+$', r'^[\s\S]{0,16384}$', r'^(?:yes|no)$'])
def test_portable_regex_catalog_admission(pattern):
    value = artifact()
    value['rules']['extra'] = dict(op='regexp', pattern=pattern)
    catalog(value)


def test_resource_grammar_is_not_operation_grammar(tmp_path):
    from coglet_surface import Operation
    from test_contract_profile import surface
    s = surface(tmp_path/'resources')
    try:
        op = Operation('expression.set_expression', 'expression.control', 'action',
                       'expression::set-expression', 'surface::empty-map',
                       ('display.face', 'softmax.app.control'), lambda a,c: None, lambda a,c: {})
        with pytest.raises(ValueError):
            s.register(op)
    finally:
        s.close()
