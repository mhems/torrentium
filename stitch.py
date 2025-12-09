import sys
import os.path
import hashlib

directory = sys.argv[1]
extension = sys.argv[2]
n = int(sys.argv[3])

outfile = f'{directory}.{extension}'

basename = os.path.basename(directory)

with open(outfile, 'rb') as fp:
    data = fp.read()
sha256_hash = hashlib.sha256(data).hexdigest()
print(f'SHA256 hash of {outfile}: {sha256_hash}')

sys.exit(0)

with open(outfile, 'wb') as out:
    for i in range(n):
        with open(f'{os.path.join(basename, basename)}.{i}.bin', 'rb') as fp:
            bytes = fp.read()
        out.write(bytes)
print(f'{n} files concatenated to form {outfile}')
