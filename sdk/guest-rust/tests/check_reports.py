import json
import pathlib
import xml.etree.ElementTree as ET
root = pathlib.Path(__file__).resolve().parents[3]
cases = json.loads((root/'conformance/abi/reference-guests.json').read_text())
for mode in ('mock', 'wamr'):
    report = ET.parse(root/f'target/reference-{mode}.xml').getroot()
    refs = [c for c in report.findall('testcase') if c.get('name').startswith('reference-')]
    assert len(refs) == len(cases)
    assert all(c.find('skipped') is None and c.find('failure') is None for c in refs)
    print(mode, len(refs), 'reference cases passed;', report.attrib)
