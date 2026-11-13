"""Sets the LC_UUID of a 64-bit Mach-O file to a hash of its content.

Apple's linker derives the UUID of an executable from its inputs, which include the paths of
temporary object files that rustc creates under a directory with a random name, so two builds
of the same source differ in those 16 bytes and in the code-signature hash that covers them
(CI run of 8f1aab1, independent reproducible build on macos-26). This script replaces the UUID
with the first 16 bytes of the SHA-256 of the file, read with the UUID zeroed and without the
code signature; the caller signs the file again (codesign --force --sign -), which gives the
same ad-hoc signature for the same bytes.

Usage: python3 macho_uuid.py FILE
Only the Python standard library is used.
"""

import hashlib
import struct
import sys

MH_MAGIC_64 = 0xFEEDFACF
LC_UUID = 0x1B
LC_CODE_SIGNATURE = 0x1D


def main(path):
    with open(path, "rb") as f:
        data = bytearray(f.read())
    magic, _cpu, _sub, _type, ncmds, _size, _flags, _res = struct.unpack_from("<8I", data, 0)
    if magic != MH_MAGIC_64:
        sys.exit(f"{path}: not a 64-bit little-endian Mach-O file")
    offset = 32
    uuid_at = None
    signature = None
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack_from("<2I", data, offset)
        if cmd == LC_UUID:
            uuid_at = offset + 8
        elif cmd == LC_CODE_SIGNATURE:
            signature = struct.unpack_from("<2I", data, offset + 8)
        offset += cmdsize
    if uuid_at is None:
        sys.exit(f"{path}: no LC_UUID")
    data[uuid_at:uuid_at + 16] = bytes(16)
    content = data
    if signature is not None:
        dataoff, _datasize = signature
        content = data[:dataoff]
    uuid = bytearray(hashlib.sha256(content).digest()[:16])
    uuid[6] = (uuid[6] & 0x0F) | 0x50  # version 5, a name-based UUID
    uuid[8] = (uuid[8] & 0x3F) | 0x80  # the variant of RFC 4122
    data[uuid_at:uuid_at + 16] = uuid
    with open(path, "r+b") as f:
        f.write(data)
    print(f"{path}: LC_UUID {uuid.hex()}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: python3 macho_uuid.py FILE")
    main(sys.argv[1])
