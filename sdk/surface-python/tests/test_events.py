import pytest
from coglet_surface.codec import decode
from coglet_surface.surface import Surface, Operation, Rejection
from test_surface import SURFACE, COGLET, CID


def test_trusted_extension_catalog_allows_simulator_events_only_with_grants(tmp_path):
    catalog = {'sensor.tick': dict(entry='sensor.control', direction='event',
                                  input='sim:tick', output=None)}
    schemas = {'sim::tick': {'op': 'record', 'fields': [dict(name='value', optional=False,
        type=dict(op='range', min=0, max=10))]}}
    surface = Surface(SURFACE, {COGLET}, tmp_path / 's', create=True, clock=lambda: 0,
                      stop=lambda: None, extensions=catalog, schemas=schemas)
    surface.register(Operation('sensor.tick', 'sensor.control', 'event', 'sim::tick',
                              None, (), lambda args, context: None, lambda args, context: None))
    from coglet_surface.codec import encode
    surface.open(CID)
    surface.receive(encode('surface::surface-message', dict(type='hello_reply', body=dict(
        selection=dict(version='0.1'), correlation_id=CID))))
    surface.drain()
    with pytest.raises(Rejection):
        surface.event(COGLET, 'sensor.tick', {'value': 1}, epoch=1)
    surface.grant([dict(coglet=COGLET, operations=['sensor.tick'], resources=[], expires=1000)])
    surface.drain()
    surface.event(COGLET, 'sensor.tick', {'value': 2}, epoch=1)
    event = decode('surface::surface-message', surface.drain()[0])
    assert event['body']['source'] == SURFACE and event['body']['seq'] == 0
    assert event['body']['payload'] == {'value': 2}
    with pytest.raises(ValueError):
        surface.event(COGLET, 'sensor.tick', {'value': 11}, epoch=1)
    surface.remove('sensor.tick')
    result = [decode('surface::surface-message', r) for r in surface.drain()]
    assert [m['type'] for m in result] == ['grants', 'capabilities']
    assert result[1]['body']['revision'] == 2
    with pytest.raises(Rejection):
        surface.event(COGLET, 'sensor.tick', {'value': 2}, epoch=1)
    surface.close()
