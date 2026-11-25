"""Product shape corpus: positives for every rule/union branch and closed negatives."""
from copy import deepcopy

def cases(rules):
    def sample(n):
        op = n['op']
        if op == 'ref': return sample(rules[n['name']])
        if op == 'literal': return n['value']
        if op == 'primitive': return {'tstr': '', 'bool': False, 'null': None}[n['name']]
        if op == 'range': return max(0, n['min'])
        if op == 'size': return 'x' * n['min']
        if op == 'union': return sample(n['items'][0])
        if op == 'array': return [sample(n['item']) for _ in range(n['min'])]
        if op == 'record': return {f['name']: sample(f['type']) for f in n['fields'] if not f['optional']}
        if op == 'intersection':
            # CDDL .size lowers to an intersection of tstr and size.
            return sample(next(x for x in n['items'] if x['op'] == 'size'))
        raise ValueError(op)
    out = []
    def add(name, rule, valid, value):
        out.append(dict(name=name, rule=rule, valid=valid, value=value))
    for rule, node in rules.items():
        add(rule + '-positive', rule, True, sample(node))
        branches = node['items'] if node['op'] == 'union' else [node]
        for i, branch in enumerate(branches):
            value = sample(branch)
            add(f'{rule}-branch-{i}', rule, True, value)
            if isinstance(value, dict):
                add(f'{rule}-unknown-{i}', rule, False, dict(value, unknown='no'))
                for key in value:
                    missing = deepcopy(value); del missing[key]
                    add(f'{rule}-missing-{i}-{key}', rule, False, missing)
    for rule, limit in [('app-id',128), ('app-cursor',128), ('app-label',256), ('app-text',4096)]:
        for text, valid, label in [('x'*limit,True,'ascii-bound'), ('x'*(limit+1),False,'ascii-over'),
                                   ('é'*(limit//2),True,'utf8-bound'), ('é'*(limit//2)+'x',False,'utf8-over')]:
            add(rule+'-'+label,rule,valid,text)
    for limit in (0,129):
        add('page-limit-'+str(limit),'app-controls-args',False,dict(window='settings',after=None,limit=limit,revision=None))
    add('cursor-without-revision','app-controls-args',False,dict(window='settings',after='cursor',limit=1,revision=None))
    add('revision-without-cursor','app-controls-args',False,dict(window='settings',after=None,limit=1,revision=3))
    add('bad-window-route','app-location',False,dict(window='about',route='privacy'))
    add('unknown-window','app-window-args',False,dict(window='arbitrary'))
    add('closed-window-route','app-window-changed',False,dict(revision=1,window='settings',open=False,route='general'))
    add('unknown-setting','app-setting-args',False,dict(key='provider_token',value='secret'))
    add('setting-sibling-type','app-setting-args',False,dict(key='talk_mode',value=True))
    add('setting-bool-as-int','app-setting-args',False,dict(key='floating_orb',value=1))
    add('redaction-no-content','app-value',False,dict(kind='redacted',value='secret'))
    add('redaction-no-length','app-value',False,dict(kind='redacted',length=0))
    add('select-not-free-text','app-invoke-args',False,dict(verb='select',window='settings',control='appearance',value='arbitrary'))
    add('press-no-value','app-invoke-args',False,dict(verb='press',window='settings',control='button',value=True))
    add('uint64-max','app-revision',True,18446744073709551615)
    add('negative-revision','app-revision',False,-1)
    add('bool-not-integer','app-revision',False,True)
    add('uint64-over','app-revision',False,18446744073709551616)
    add('elapsed-over','app-ptt',False,dict(state='listening',visible=True,elapsed_ms=120001,level=1))
    add('level-over','app-hud',False,dict(state='listening',visible=True,elapsed_ms=1,level=1001))
    add('redaction-not-settable','app-invoke-args',False,dict(verb='set_value',window='settings',control='field',value=dict(kind='redacted')))
    control = sample(rules['app-control'])
    for count, valid in [(128,True),(129,False)]:
        add('controls-page-'+str(count),'app-controls-result',valid,dict(revision=1,window='settings',controls=[control]*count,next=None))
    for count, valid in [(64,True),(65,False)]:
        value = sample(rules['app-state-result']); value['windows']=[dict(open=False,window='main',route=None)]*count
        add('windows-bound-'+str(count),'app-state-result',valid,value)
    return out
