"""Python SDK sandbox adapter. One fresh ledger and pilot per episode.

A simulator restart must start a new episode/ledger: this adapter deliberately
refuses reopening a ledger rather than pairing old receipts with a reset world.
Actions and observations use the real SDK's validation/grants/durable receipts.
"""
from pathlib import Path

from coglet_surface.codec import validate
from coglet_surface.surface import Operation, Rejection, Surface

from .worlds import Cartpole, Gridbot


def _pilot(coglets):
    coglets = tuple(coglets)
    if len(coglets) != 1:
        raise ValueError('sandbox requires exactly one pilot identity')
    validate('common::coglet-id', coglets[0])
    return coglets


def runtime_binding(identity, coglets):
    validate('common::surface-id', identity)
    return dict(surface=identity, permitted_coglets=list(_pilot(coglets)))


class WorldSurface(Surface):
    def __init__(self, identity, coglets, ledger, world, *, clock=None):
        coglets = _pilot(coglets)
        if type(world) not in (Gridbot, Cartpole):
            raise ValueError('unsupported sandbox world')
        if Path(ledger).exists():
            raise ValueError('new episode requires a fresh ledger')
        self.world = world
        self.pilot = coglets[0]
        name = 'gridbot' if type(world) is Gridbot else 'cartpole'
        self.event_name = f'sim.{name}_state'
        self.action_name = f'sim.{name}_' + ('move' if name == 'gridbot' else 'force')
        super().__init__(identity, coglets, ledger, create=True, clock=clock, stop=lambda:None)
        self.register(Operation(self.event_name, f'sim.{name}', 'event',
            f'sandbox::{name}-state', None, (), lambda args, context:None, lambda args, context:None))
        self.register(Operation(self.action_name, f'sim.{name}', 'action',
            'sandbox::gridbot-move' if name == 'gridbot' else 'sandbox::cartpole-force',
            'surface::empty-map', (), self._prepare, self._execute))

    def _prepare(self, args, context):
        if self.world.done:
            raise Rejection('conflict', 'episode terminated')

    def _execute(self, args, context):
        self.world.step(args['direction'])
        self.publish(epoch=context.epoch)
        return {}

    def publish(self, *, epoch):
        payload = self.world.snapshot()
        for key in ('world', 'seed', 'max_steps'):
            payload.pop(key, None)
        payload['state_hash'] = self.world.state_hash()
        self.event(self.pilot, self.event_name, payload, epoch=epoch)
