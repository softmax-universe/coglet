from coglet_surface.codec import encode
from coglet_surface.surface import Operation
from test_surface import make_surface, act, CID, COGLET


def test_handler_receives_immutable_admission_context(tmp_path):
    surface, _ = make_surface(tmp_path)
    op = surface.operations['expression.set_expression']
    seen = []
    surface.operations[op.name] = Operation(op.name, op.entry, op.direction, op.input,
        op.output, op.resources, lambda args, context: None,
        lambda args, context: seen.append(context) or {})
    result = act(surface)
    assert result[1]['body']['payload']['status'] == 'succeeded'
    assert seen[0].coglet == COGLET and seen[0].action_id == CID and seen[0].epoch == 1
    surface.close()
