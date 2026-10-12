#!/usr/bin/env python3
"""Register a throwaway Bitwarden-protocol account on a local Vaultwarden.

Usage: register.py BASE_URL EMAIL PASSWORD
Implements the client-side key derivation (PBKDF2-SHA256, HKDF stretch, AES-256-CBC +
HMAC EncString type 2, RSA-2048 keypair) so no browser is needed for sign-up.
"""
import base64
import json
import os
import ssl
import sys
import urllib.request

from cryptography.hazmat.primitives import hashes, hmac, padding, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.hkdf import HKDFExpand
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC

# The QA Vaultwarden serves a throw-away self-signed certificate on 127.0.0.1 (the Bitwarden clients insist on HTTPS).
_LOCAL_TLS = ssl.create_default_context()
_LOCAL_TLS.check_hostname = False
_LOCAL_TLS.verify_mode = ssl.CERT_NONE
urllib.request.install_opener(urllib.request.build_opener(urllib.request.HTTPSHandler(context=_LOCAL_TLS)))

KDF_ITERATIONS = 600_000


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode()


def pbkdf2(password: bytes, salt: bytes, iterations: int) -> bytes:
    return PBKDF2HMAC(hashes.SHA256(), 32, salt, iterations).derive(password)


def enc_string(enc_key: bytes, mac_key: bytes, plain: bytes) -> str:
    iv = os.urandom(16)
    padder = padding.PKCS7(128).padder()
    padded = padder.update(plain) + padder.finalize()
    encryptor = Cipher(algorithms.AES(enc_key), modes.CBC(iv)).encryptor()
    cipher = encryptor.update(padded) + encryptor.finalize()
    mac = hmac.HMAC(mac_key, hashes.SHA256())
    mac.update(iv + cipher)
    return f"2.{b64(iv)}|{b64(cipher)}|{b64(mac.finalize())}"


def main() -> None:
    base, email, password = sys.argv[1].rstrip("/"), sys.argv[2].lower(), sys.argv[3]
    master_key = pbkdf2(password.encode(), email.encode(), KDF_ITERATIONS)
    master_hash = b64(pbkdf2(master_key, password.encode(), 1))
    stretched_enc = HKDFExpand(hashes.SHA256(), 32, b"enc").derive(master_key)
    stretched_mac = HKDFExpand(hashes.SHA256(), 32, b"mac").derive(master_key)
    sym_key = os.urandom(64)
    protected_key = enc_string(stretched_enc, stretched_mac, sym_key)
    private = rsa.generate_private_key(65537, 2048)
    public_der = private.public_key().public_bytes(
        serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo
    )
    private_der = private.private_bytes(
        serialization.Encoding.DER,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    token_request = urllib.request.Request(
        f"{base}/identity/accounts/register/send-verification-email",
        data=json.dumps({"email": email, "name": "openside-qa"}).encode(),
        headers={"Content-Type": "application/json", "Accept": "application/json"},
    )
    # Mail is disabled on the throwaway server, so it returns the verification JWT directly.
    with urllib.request.urlopen(token_request, timeout=30) as response:
        verification_token = json.load(response)
    body = {
        "email": email,
        "emailVerificationToken": verification_token,
        "masterPasswordHash": master_hash,
        "masterPasswordHint": None,
        "key": protected_key,
        "userAsymmetricKeys": {
            "publicKey": b64(public_der),
            "encryptedPrivateKey": enc_string(sym_key[:32], sym_key[32:], private_der),
        },
        "kdf": 0,
        "kdfIterations": KDF_ITERATIONS,
    }
    request = urllib.request.Request(
        f"{base}/identity/accounts/register/finish",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            print("register:", response.status)
    except urllib.error.HTTPError as error:
        print("register failed:", error.code, error.read()[:300].decode(errors="replace"))
        sys.exit(1)


if __name__ == "__main__":
    main()
