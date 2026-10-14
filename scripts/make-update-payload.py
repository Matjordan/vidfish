#!/usr/bin/env python3
"""One manifest and complete payload for the same compiled release version."""
import hashlib,json,pathlib,sys,zipfile
root=pathlib.Path(sys.argv[1]); version=sys.argv[2]; platform=sys.argv[3]
files={str(p.relative_to(root)).replace('\\','/'):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(root.rglob('*')) if p.is_file() and p.name!='bundle.json'}
(root/'bundle.json').write_text(json.dumps(dict(schema=1,version=version,platform=platform,files=files),indent=2)+'\n')
with zipfile.ZipFile(f'vidfish-{version}-{platform}-update.zip','w',zipfile.ZIP_DEFLATED) as z:
    for p in sorted(root.rglob('*')):
        if p.is_file(): z.write(p,str(p.relative_to(root)))
