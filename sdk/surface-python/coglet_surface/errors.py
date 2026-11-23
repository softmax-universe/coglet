class ProtocolError(ValueError):
    """Fatal wire/state/storage violation; disconnect rather than dispatch."""
