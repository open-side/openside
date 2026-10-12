#!/usr/bin/env python3
"""Add one login item to a local Vaultwarden account (the `bw` CLI refuses http:// servers).

Usage: add_login.py BASE_URL EMAIL PASSWORD ITEM_NAME URI USERNAME ITEM_PASSWORD
"""
import base64
import json
import os
import ssl
import sys
import urllib.parse
import urllib.request
import uuid

from cryptography.hazmat.primitives import hashes, hmac, padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.hkdf import HKDFExpand
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC

# The QA Vaultwarden serves a throw-away self-signed certificate on 127.0.0.1 (the Bitwarden clients insist on HTTPS).
_LOCAL_TLS = ssl.create_default_context()
_LOCAL_TLS.check_hostname = False
_LOCAL_TLS.verify_mode = ssl.CERT_NONE
urllib.request.install_opener(urllib.request.build_opener(urllib.request.HTTPSHandler(context=_LOCAL_TLS)))


def pbkdf2(password: bytes, salt: bytes, iterations: int) -> bytes:
    return PBKDF2HMAC(hashes.SHA256(), 32, salt, iterations).derive(password)


def enc_string(enc_key: bytes, mac_key: bytes, plain: str) -> str:
    iv = os.urandom(16)
    padder = padding.PKCS7(128).padder()
    padded = padder.update(plain.encode()) + padder.finalize()
    encryptor = Cipher(algorithms.AES(enc_key), modes.CBC(iv)).encryptor()
    cipher = encryptor.update(padded) + encryptor.finalize()
    mac = hmac.HMAC(mac_key, hashes.SHA256())
    mac.update(iv + cipher)
    parts = [base64.b64encode(part).decode() for part in (iv, cipher, mac.finalize())]
    return "2." + "|".join(parts)


def dec_key(enc_key: bytes, mac_key: bytes, text: str) -> bytes:
    iv, cipher, mac = (base64.b64decode(part) for part in text.split(".", 1)[1].split("|"))
    check = hmac.HMAC(mac_key, hashes.SHA256())
    check.update(iv + cipher)
    check.verify(mac)
    decryptor = Cipher(algorithms.AES(enc_key), modes.CBC(iv)).decryptor()
    unpadder = padding.PKCS7(128).unpadder()
    return unpadder.update(decryptor.update(cipher) + decryptor.finalize()) + unpadder.finalize()


def main() -> None:
    base, email, password, name, uri, username, item_password = sys.argv[1:8]
    base = base.rstrip("/")
    email = email.lower()
    master_key = pbkdf2(password.encode(), email.encode(), 600_000)
    master_hash = base64.b64encode(pbkdf2(master_key, password.encode(), 1)).decode()
    form = urllib.parse.urlencode(
        {
            "grant_type": "password",
            "username": email,
            "password": master_hash,
            "scope": "api offline_access",
            "client_id": "web",
            "deviceType": "9",
            "deviceIdentifier": str(uuid.uuid4()),
            "deviceName": "openside-qa-script",
        }
    ).encode()
    with urllib.request.urlopen(f"{base}/identity/connect/token", data=form, timeout=30) as response:
        token = json.load(response)
    stretched_enc = HKDFExpand(hashes.SHA256(), 32, b"enc").derive(master_key)
    stretched_mac = HKDFExpand(hashes.SHA256(), 32, b"mac").derive(master_key)
    sym = dec_key(stretched_enc, stretched_mac, token["Key"])
    enc = lambda text: enc_string(sym[:32], sym[32:], text)
    profile = urllib.request.Request(
        f"{base}/api/accounts/profile", headers={"Authorization": f"Bearer {token['access_token']}"}
    )
    with urllib.request.urlopen(profile, timeout=30) as response:
        user_id = json.load(response)["id"]
    body = {
        "encryptedFor": user_id,
        "type": 1,
        "name": enc(name),
        "notes": None,
        "favorite": False,
        "reprompt": 0,
        "login": {
            "uris": [{"uri": enc(uri), "match": None}],
            "username": enc(username),
            "password": enc(item_password),
            "totp": None,
        },
    }
    request = urllib.request.Request(
        f"{base}/api/ciphers",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {token['access_token']}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            print("created cipher:", json.load(response)["id"])
    except urllib.error.HTTPError as error:
        print("create failed:", error.code, error.read()[:400].decode(errors="replace"))
        sys.exit(1)


if __name__ == "__main__":
    main()
