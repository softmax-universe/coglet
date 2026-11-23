"""Common deterministic CBOR profile and bounded transport framing (no dependencies)."""
import asyncio
import struct

MAX_FRAME = 4 * 1024 * 1024


def head(major, value):
    for limit, tag, fmt in [(24, None, None), (256, 24, '>B'),
                            (65536, 25, '>H'), (2**32, 26, '>I'), (2**64, 27, '>Q')]:
        if value < limit:
            return bytes([(major << 5) | (value if tag is None else tag)]) + (
                b'' if fmt is None else struct.pack(fmt, value))
    raise ValueError('integer out of range')


def encode(value):
    if value is None:
        return b'\xf6'
    if type(value) is bool:
        return b'\xf5' if value else b'\xf4'
    if type(value) is int and -(2**63) <= value < 2**64:
        return head(0, value) if value >= 0 else head(1, -1 - value)
    if isinstance(value, str):
        raw = value.encode('utf-8')
        return head(3, len(raw)) + raw
    if isinstance(value, list):
        return head(4, len(value)) + b''.join(encode(v) for v in value)
    if isinstance(value, dict) and all(isinstance(k, str) for k in value):
        pairs = sorted((encode(k), encode(v)) for k, v in value.items())
        return head(5, len(pairs)) + b''.join(k + v for k, v in pairs)
    raise ValueError('value outside conventions CBOR profile')


def decode(raw):
    offset = 0

    def item(depth=0):
        nonlocal offset
        if depth > 128 or offset >= len(raw):
            raise ValueError('truncated or overly nested CBOR')
        initial = raw[offset]
        offset += 1
        major, arg = initial >> 5, initial & 31
        if major == 7:
            if initial not in (0xf4, 0xf5, 0xf6):
                raise ValueError('unsupported simple value')
            return {0xf4: False, 0xf5: True, 0xf6: None}[initial]
        if arg >= 24:
            sizes = {24: 1, 25: 2, 26: 4, 27: 8}
            if arg not in sizes or offset + sizes[arg] > len(raw):
                raise ValueError('invalid CBOR length')
            size = sizes[arg]
            arg = int.from_bytes(raw[offset:offset + size], 'big')
            offset += size
        if major == 0:
            return arg
        if major == 1:
            return -1 - arg
        if major == 3:
            if offset + arg > len(raw):
                raise ValueError('truncated text')
            text = raw[offset:offset + arg].decode('utf-8')
            offset += arg
            return text
        if major == 4:
            if arg > len(raw) - offset:
                raise ValueError('truncated array')
            return [item(depth + 1) for _ in range(arg)]
        if major == 5:
            if arg > (len(raw) - offset) // 2:
                raise ValueError('truncated map')
            result = {}
            for _ in range(arg):
                key = item(depth + 1)
                if not isinstance(key, str) or key in result:
                    raise ValueError('non-text or duplicate map key')
                result[key] = item(depth + 1)
            return result
        raise ValueError('unsupported CBOR major type')

    value = item()
    if offset != len(raw) or encode(value) != raw:
        raise ValueError('not a single deterministic CBOR profile item')
    return value


async def read_frame(reader, framing='u32be'):
    if framing == 'u32be':
        size = int.from_bytes(await reader.readexactly(4), 'big')
        if not 0 < size <= MAX_FRAME:
            raise ValueError(f'invalid frame length {size}')
        return await reader.readexactly(size)
    if framing == 'cbor-sequence':
        raw = bytearray()

        async def take(size):
            if size > MAX_FRAME - len(raw):
                raise ValueError('CBOR item exceeds frame limit')
            part = await reader.readexactly(size)
            raw.extend(part)
            return part

        async def item(depth=0):
            if depth > 128:
                raise ValueError('overly nested CBOR')
            initial = (await take(1))[0]
            major, arg = initial >> 5, initial & 31
            if major == 7:
                if initial not in (0xf4, 0xf5, 0xf6):
                    raise ValueError('unsupported simple value')
                return
            if major not in (0, 1, 3, 4, 5):
                raise ValueError('unsupported CBOR major type')
            if arg >= 24:
                size = {24: 1, 25: 2, 26: 4, 27: 8}.get(arg)
                if size is None:
                    raise ValueError('invalid CBOR length')
                arg = int.from_bytes(await take(size), 'big')
            if major == 3:
                await take(arg)
            if major in (4, 5):
                count = arg if major == 4 else 2 * arg
                if count > MAX_FRAME - len(raw):
                    raise ValueError('container exceeds frame limit')
                for _ in range(count):
                    await item(depth + 1)

        await item()
        decode(bytes(raw))
        return bytes(raw)
    raise ValueError(f'unknown framing {framing!r}')


async def write_frame(writer, raw, framing='u32be'):
    if not 0 < len(raw) <= MAX_FRAME:
        raise ValueError('invalid frame size')
    if framing not in ('u32be', 'cbor-sequence'):
        raise ValueError('unknown framing')
    writer.write((len(raw).to_bytes(4, 'big') if framing == 'u32be' else b'') + raw)
    await writer.drain()
