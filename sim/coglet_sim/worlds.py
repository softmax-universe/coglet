"""Seeded, action-clocked M2 worlds. No wall time or ambient randomness."""
from dataclasses import dataclass
from hashlib import sha256

from coglet_surface.codec import encode_profile


class World:
    def state_hash(self):
        return sha256(encode_profile(self.snapshot())).hexdigest()


def _integer(value, low, high, name):
    if type(value) is not int or not low <= value <= high:
        raise ValueError(f'{name} must be an integer in [{low}, {high}]')
    return value


def _samples(seed):
    _integer(seed, 0, 2**64 - 1, 'seed')
    for _ in range(4):
        seed = (6364136223846793005 * seed + 1442695040888963407) % 2**64
        yield seed


@dataclass(init=False)
class Gridbot(World):
    seed: int
    max_steps: int
    width: int
    height: int
    x: int
    y: int
    goal_x: int
    goal_y: int
    steps: int
    done: bool

    def __init__(self, seed, *, width=8, height=8, max_steps=10000):
        self.max_steps = _integer(max_steps, 1, 10000, 'max_steps')
        self.width = _integer(width, 2, 64, 'width')
        self.height = _integer(height, 2, 64, 'height')
        a, b, c, d = _samples(seed)
        self.seed = seed
        self.x, self.y = a % width, b % height
        self.goal_x, self.goal_y = c % width, d % height
        if (self.x, self.y) == (self.goal_x, self.goal_y):
            self.goal_x = (self.goal_x + 1) % width
        self.steps, self.done = 0, False

    def snapshot(self):
        return dict(world='gridbot-v1', seed=self.seed, max_steps=self.max_steps, width=self.width,
                    height=self.height, x=self.x, y=self.y, goal_x=self.goal_x,
                    goal_y=self.goal_y, steps=self.steps, done=self.done)

    def step(self, direction):
        moves = dict(north=(0, -1), south=(0, 1), east=(1, 0), west=(-1, 0))
        if not isinstance(direction, str) or direction not in moves:
            raise ValueError('invalid direction')
        if self.done:
            raise ValueError('episode terminated')
        dx, dy = moves[direction]
        self.x = min(self.width - 1, max(0, self.x + dx))
        self.y = min(self.height - 1, max(0, self.y + dy))
        self.steps += 1
        self.done = ((self.x, self.y) == (self.goal_x, self.goal_y)
                     or self.steps >= self.max_steps)
        return self.snapshot()


SCALE = 1_000_000


def _trunc(numerator, denominator):
    """Integer division toward zero, independent of Python's negative // rule."""
    quotient = abs(numerator) // abs(denominator)
    return -quotient if (numerator < 0) != (denominator < 0) else quotient


def _mul(a, b):
    return _trunc(a * b, SCALE)


def _div(a, b):
    return _trunc(a * SCALE, b)


@dataclass(init=False)
class Cartpole(World):
    seed: int
    max_steps: int
    x: int
    velocity: int
    angle: int
    angular_velocity: int
    steps: int
    done: bool

    def __init__(self, seed, *, max_steps=500):
        self.max_steps = _integer(max_steps, 1, 10000, 'max_steps')
        self.x, self.velocity, self.angle, self.angular_velocity = (
            s % 100001 - 50000 for s in _samples(seed))
        self.seed, self.steps, self.done = seed, 0, False

    def snapshot(self):
        return dict(world='cartpole-v1', seed=self.seed, max_steps=self.max_steps,
                    x=self.x, velocity=self.velocity, angle=self.angle,
                    angular_velocity=self.angular_velocity, steps=self.steps,
                    done=self.done)

    def step(self, direction):
        if not isinstance(direction, str) or direction not in ('left', 'right'):
            raise ValueError('invalid direction')
        if self.done:
            raise ValueError('episode terminated')
        t = self.angle
        t2 = _mul(t, t)
        t3, t4 = _mul(t2, t), _mul(t2, t2)
        t5 = _mul(t4, t)
        sine = t - _trunc(t3, 6) + _trunc(t5, 120)
        cosine = SCALE - _trunc(t2, 2) + _trunc(t4, 24)
        force = 10 * SCALE if direction == 'right' else -10 * SCALE
        # Fixed point form of the standard frictionless cartpole equations.
        pole_mass_length, total_mass, half_length = 50000, 1100000, 500000
        temp = _div(force + _mul(_mul(pole_mass_length,
                        _mul(self.angular_velocity, self.angular_velocity)), sine), total_mass)
        angular_acceleration = _div(_mul(9800000, sine) - _mul(cosine, temp),
            _mul(half_length, _trunc(4 * SCALE, 3)
                 - _div(_mul(100000, _mul(cosine, cosine)), total_mass)))
        acceleration = temp - _div(_mul(_mul(pole_mass_length,
                                           angular_acceleration), cosine), total_mass)
        self.x += _mul(20000, self.velocity)
        self.angle += _mul(20000, self.angular_velocity)
        self.velocity += _mul(20000, acceleration)
        self.angular_velocity += _mul(20000, angular_acceleration)
        self.steps += 1
        self.done = (abs(self.x) > 2400000 or abs(self.angle) > 209440
                     or self.steps >= self.max_steps)
        return self.snapshot()
