"""ADR-0046 spec oracle only; trusted inputs are not network authority proofs."""


class Denied(ValueError):
    pass


def admit(request, state):
    """Evaluate a route against independently supplied authoritative test state.

    This is intentionally not a serving router or signature/iroh verifier.
    State represents snapshots that a real receiver must acquire durably and
    revalidate at every publication boundary.
    """
    def require(condition, code='permission_denied'):
        if not condition:
            raise Denied(code)

    require(state['mutual_enrollment'] and
            state['transport_peer'] == request['sender'] and
            state['local_router'] == request['receiver'] and
            request['sender'] != request['receiver'], 'unauthenticated')
    require(request['household'] == state['household'])
    require(state['attending'] in (request['sender'], request['receiver']))
    require(state['origin_binding'] == [request['sender'], request['origin'], request['actor'], request.get('ticket')])
    route = [request[k] for k in ('household', 'sender', 'receiver', 'origin', 'target', 'capability')]
    require(route == state['route'])
    require(not state['killed'] and state['authority_current'])
    require(state['clock_mapped'] and state['now'] < request['deadline'], 'deadline_exceeded')
    operation = request['capability']
    for layer in ('sending_policy', 'receiving_policy', 'delegation', 'adapter_grants'):
        require(operation in state[layer])
    # Classification is retained receiver provenance, never a caller flag.
    retained = state['private_record']
    if retained is not None:
        require(retained['epoch'] >= 1)
        require(request.get('private') == retained)
        require(state['actor_owner'] == [request['actor'], retained['owner'], retained['source'], retained['epoch']])
        require(request['target'] == retained['owner'] and state['private_active'])
    else:
        require('private' not in request)
    return 'accepted'
