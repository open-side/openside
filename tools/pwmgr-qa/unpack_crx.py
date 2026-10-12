#!/usr/bin/env python3
"""Unpack a CRX3 file and pin the extension id by writing the CRX public key into manifest.json.

Usage: unpack_crx.py FILE.crx OUT_DIR EXPECTED_EXTENSION_ID
An unpacked extension without `key` gets a path-derived id, which would never match the id a
vendor's native-messaging manifest allows; the key makes the unpacked copy behave like the store one.
"""
import base64
import hashlib
import io
import json
import struct
import sys
import zipfile


def varint(data: bytes, pos: int):
    shift = value = 0
    while True:
        byte = data[pos]
        pos += 1
        value |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return value, pos
        shift += 7


def fields(data: bytes):
    pos = 0
    while pos < len(data):
        tag, pos = varint(data, pos)
        number, wire = tag >> 3, tag & 7
        if wire == 2:
            length, pos = varint(data, pos)
            yield number, data[pos : pos + length]
            pos += length
        elif wire == 0:
            _, pos = varint(data, pos)
        else:
            raise ValueError(f"unsupported wire type {wire}")


def extension_id(public_key: bytes) -> str:
    return "".join(chr(ord("a") + int(nibble, 16)) for nibble in hashlib.sha256(public_key).hexdigest()[:32])


def main() -> None:
    crx, out, expected = sys.argv[1:4]
    data = open(crx, "rb").read()
    assert data[:4] == b"Cr24" and struct.unpack("<I", data[4:8])[0] == 3
    header_len = struct.unpack("<I", data[8:12])[0]
    header = data[12 : 12 + header_len]
    key = None
    for number, payload in fields(header):
        if number in (2, 3):  # sha256_with_rsa / sha256_with_ecdsa proofs
            for inner, blob in fields(payload):
                if inner == 1 and extension_id(blob) == expected:
                    key = blob
    if key is None:
        sys.exit(f"no public key in the CRX header hashes to {expected}")
    zipfile.ZipFile(io.BytesIO(data[12 + header_len :])).extractall(out)
    path = f"{out}/manifest.json"
    manifest = json.load(open(path))
    manifest["key"] = base64.b64encode(key).decode()
    json.dump(manifest, open(path, "w"))
    print(f"{manifest.get('version')} id={expected}")


if __name__ == "__main__":
    main()
