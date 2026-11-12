#!/usr/bin/env python3
"""Writes the expected output (`*.out`) of the `std` crypto tests from Python's hashlib, hmac and friends.

    python3 compiler/std/tests/gen_crypto.py
"""
import hashlib, os

here = os.path.dirname(os.path.abspath(__file__))

def pattern(n):
    return bytes((i * 7 + 3) & 255 for i in range(n))

def sha2():
    out = []
    lens = [0, 1, 3, 55, 56, 57, 63, 64, 65, 111, 112, 113, 119, 127, 128, 129, 200, 255, 256, 1000]
    for n in lens:
        m = pattern(n)
        for name in ["sha224", "sha256", "sha384", "sha512", "sha512_224", "sha512_256"]:
            out.append(f"{n} {hashlib.new(name, m).hexdigest()}")
    out.append(hashlib.sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").hexdigest())
    out.append(hashlib.sha512(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu").hexdigest())
    out.append("true")
    out.append(hashlib.sha256(b"a" * 3000).hexdigest())
    out.append(hashlib.sha512(b"a" * 3000).hexdigest())
    out.append("true false false")
    out.append("some([0, 255, 16]) none none")
    return out

import hmac

def pat(n, k):
    return bytes((i * k + 11) & 255 for i in range(n))

def hkdf(h, salt, ikm, info, length):
    n = h().digest_size
    prk = hmac.new(salt or b"\0" * n, ikm, h).digest()
    out, prev, c = b"", b"", 1
    while len(out) < length:
        prev = hmac.new(prk, prev + info + bytes([c]), h).digest()
        out += prev
        c += 1
    return out[:length]

def mac():
    out = []
    for kl in [0, 20, 64, 65, 129]:
        for ml in [0, 5, 100, 130]:
            k, m = pat(kl, 3), pat(ml, 5)
            out.append(f"{kl} {ml} {hmac.new(k, m, hashlib.sha256).hexdigest()} {hmac.new(k, m, hashlib.sha512).hexdigest()}")
    out.append(hmac.new(b"Jefe", b"what do ya want for nothing?", hashlib.sha256).hexdigest())
    out += ["true", "false"]
    ikm = bytes.fromhex("0b" * 22)
    salt = bytes.fromhex("000102030405060708090a0b0c")
    info = bytes.fromhex("f0f1f2f3f4f5f6f7f8f9")
    out.append(hmac.new(salt, ikm, hashlib.sha256).hexdigest())
    out.append(hkdf(hashlib.sha256, salt, ikm, info, 42).hex())
    out.append(hkdf(hashlib.sha256, b"", ikm, b"", 42).hex())
    out.append(hkdf(hashlib.sha512, salt, ikm, info, 100).hex())
    out.append(hashlib.pbkdf2_hmac("sha256", b"password", b"salt", 1, 32).hex())
    out.append(hashlib.pbkdf2_hmac("sha256", b"password", b"salt", 2, 40).hex())
    out.append(hashlib.pbkdf2_hmac("sha512", b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 3, 70).hex())
    return out

TESTS = {"sha2": sha2, "mac": mac}

if __name__ == "__main__":
    for name, f in TESTS.items():
        with open(os.path.join(here, name + ".out"), "w") as fh:
            fh.write("\n".join(f()) + "\n")
