"""Explicit selected normal Cargo-graph gate, NOT a universal no-syscall claim.

Run against the exact checkout before building/pinning its bare hosted driver.
The supervisor preparation crate is deliberately outside this graph.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess

FORBIDDEN={'coglet-cli','coglet-proto','coglet-llm','reqwest','hyper','hyper-util',
           'socket2','mio','quinn','quinn-proto','hickory-resolver','tokio-websockets',
           'tokio-tungstenite','tungstenite','netwatch','netdev','noq','noq-proto','noq-udp'}
ROW=re.compile(r'^([a-zA-Z0-9_-]+) v\S+(?: \([^\n]*\))? features=([^\s]*)(?: \(\*\))?$')

def verify(text):
    packages=set();runtime=[]
    for line in text.splitlines():
        match=ROW.fullmatch(line)
        if not match:raise ValueError('unparseable cargo graph row')
        name,features=match.groups();packages.add(name)
        if name=='coglet-runtime':runtime.append(set(filter(None,features.split(','))))
    if not {'coglet-unforged-proof','coglet-visualizer-native','coglet-runtime'}<=packages:
        raise ValueError('incomplete driver graph')
    forbidden=sorted(n for n in packages if n in FORBIDDEN or n=='iroh' or n.startswith('iroh-'))
    if forbidden:raise ValueError('live transport dependencies: '+','.join(forbidden))
    if not runtime or any('native-engine' not in f or {'device','default'}&f for f in runtime):
        raise ValueError('runtime features are not native-only')
    return dict(packages=sorted(packages),runtime_features=[sorted(f) for f in runtime],
                claim='selected normal dependency graph only; source audit and actual hosted proof separate')

def inspect(cargo='cargo',toolchain='+1.100.0'):
    root=Path(__file__).resolve().parent
    command=[cargo,toolchain,'tree','--locked','--offline','--manifest-path',str(root/'driver/Cargo.toml'),
             '--target','x86_64-unknown-linux-gnu','--edges','normal','--prefix','none','--format','{p} features={f}']
    run=subprocess.run(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True,text=True,timeout=120)
    result=verify(run.stdout)
    return dict(command=command,raw_graph=run.stdout,verification=result)

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args();result=inspect()
    with args.output.open('x') as out:json.dump(result,out,indent=2);out.write('\n')
    print(json.dumps(result['verification'],indent=2))

if __name__=='__main__':main()
