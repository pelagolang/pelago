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

import zlib

def keccak_f(a):
    rc = [0x0000000000000001, 0x0000000000008082, 0x800000000000808a, 0x8000000080008000, 0x000000000000808b, 0x0000000080000001,
          0x8000000080008081, 0x8000000000008009, 0x000000000000008a, 0x0000000000000088, 0x0000000080008009, 0x000000008000000a,
          0x000000008000808b, 0x800000000000008b, 0x8000000000008089, 0x8000000000008003, 0x8000000000008002, 0x8000000000000080,
          0x000000000000800a, 0x800000008000000a, 0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008]
    M = (1 << 64) - 1
    rot = lambda x, n: ((x << n) | (x >> (64 - n))) & M if n else x
    # the textbook way: lane (x, y), rho offsets by walking the (x, y) -> (y, 2x + 3y) path
    r = {}
    x, y = 1, 0
    r[(0, 0)] = 0
    for t in range(24):
        r[(x, y)] = ((t + 1) * (t + 2) // 2) % 64
        x, y = y, (2 * x + 3 * y) % 5
    A = {(x, y): a[x + 5 * y] for x in range(5) for y in range(5)}
    for rnd in range(24):
        C = [A[(x, 0)] ^ A[(x, 1)] ^ A[(x, 2)] ^ A[(x, 3)] ^ A[(x, 4)] for x in range(5)]
        D = [C[(x - 1) % 5] ^ rot(C[(x + 1) % 5], 1) for x in range(5)]
        A = {(x, y): A[(x, y)] ^ D[x] for x in range(5) for y in range(5)}
        B = {(y, (2 * x + 3 * y) % 5): rot(A[(x, y)], r[(x, y)]) for x in range(5) for y in range(5)}
        A = {(x, y): B[(x, y)] ^ (~B[((x + 1) % 5, y)] & B[((x + 2) % 5, y)] & M) for x in range(5) for y in range(5)}
        A[(0, 0)] ^= rc[rnd]
    return [A[(x, y)] for y in range(5) for x in range(5)]

def sponge(data, rate, suffix, outlen):
    st = [0] * 25
    data = bytearray(data) + bytes([suffix])
    while len(data) % rate:
        data.append(0)
    data[-1] |= 0x80
    for off in range(0, len(data), rate):
        for i in range(rate // 8):
            st[i] ^= int.from_bytes(data[off + 8 * i: off + 8 * i + 8], "little")
        st = keccak_f(st)
    out = b""
    while True:
        for i in range(rate // 8):
            out += st[i].to_bytes(8, "little")
        if len(out) >= outlen:
            return out[:outlen]
        st = keccak_f(st)

def sha3():
    out = []
    for n in [0, 1, 71, 72, 73, 135, 136, 137, 200]:
        m = pattern(n)
        for bits in (224, 256, 384, 512):
            h = getattr(hashlib, f"sha3_{bits}")(m).hexdigest()
            assert h == sponge(m, 200 - bits // 4, 6, bits // 8).hex()
            out.append(f"{n} {h}")
        out.append(f"{n} {sponge(m, 136, 1, 32).hex()}")
        out.append(f"{n} {sponge(m, 72, 1, 64).hex()}")
        out.append(f"{n} {hashlib.shake_128(m).hexdigest(32)}")
        out.append(f"{n} {hashlib.shake_256(m).hexdigest(64)}")
    out.append(hashlib.shake_128(pattern(10)).hexdigest(5))
    out.append(hashlib.shake_128(pattern(10)).hexdigest(200))
    out.append("true")
    return out

def legacy():
    out = []
    for n in [0, 1, 3, 55, 56, 57, 63, 64, 65, 119, 120, 128, 200, 1000]:
        m = pattern(n)
        out.append(f"{n} {hashlib.md5(m).hexdigest()} {hashlib.sha1(m).hexdigest()}")
    m = pattern(33) * 30
    out.append(f"{hashlib.md5(m).hexdigest()} {hashlib.sha1(m).hexdigest()}")
    out.append(hashlib.md5(b"The quick brown fox jumps over the lazy dog").hexdigest())
    out.append(hashlib.sha1(b"The quick brown fox jumps over the lazy dog").hexdigest())
    return out

def crc_ref(width, poly, init, reflect, xorout, data):
    """Bit by bit, straight from the definition (no tables)."""
    mask = (1 << width) - 1
    def rev(v, w):
        return int(format(v, f"0{w}b")[::-1], 2)
    reg = init
    for b in data:
        if reflect:
            b = rev(b, 8)
        for i in range(7, -1, -1):
            bit = ((b >> i) & 1) ^ ((reg >> (width - 1)) & 1)
            reg = (reg << 1) & mask
            if bit:
                reg ^= poly
    if reflect:
        reg = rev(reg, width)
    return (reg ^ xorout) & mask

def adler(data):
    a, b = 1, 0
    for x in data:
        a = (a + x) % 65521
        b = (b + a) % 65521
    return (b << 16) | a

def checksum():
    out = []
    params = [(32, 0x04c11db7, 0xffffffff, True, 0xffffffff), (32, 0x1edc6f41, 0xffffffff, True, 0xffffffff),
              (64, 0x42f0e1eba9ea3693, (1 << 64) - 1, True, (1 << 64) - 1), (64, 0x42f0e1eba9ea3693, 0, False, 0)]
    params2 = [(16, 0x8005, 0, True, 0), (16, 0x1021, 0xffff, False, 0), (16, 0x1021, 0, False, 0), (8, 0x07, 0, False, 0)]
    for m in [b"123456789", b"", pattern(1), pattern(100), pattern(5553)]:
        out.append(" ".join(f"{crc_ref(*p, m):0{p[0] // 4}x}" for p in params))
        out.append(" ".join(f"{crc_ref(*p, m):0{p[0] // 4}x}" for p in params2) + f" {adler(m):08x}")
        assert crc_ref(*params[0], m) == zlib.crc32(m) and adler(m) == zlib.adler32(m)
    out.append(f"{crc_ref(24, 0x864cfb, 0xb704ce, False, 0, b'123456789'):06x}")
    out.append(f"{crc_ref(40, 0x0004820009, 0, False, 0xffffffffff, b'123456789'):010x}")
    out.append(f"{zlib.crc32(b'123456789')} true")
    out.append(str(zlib.adler32(b"Wikipedia")))
    return out

TESTS = {"sha2": sha2, "mac": mac, "sha3": sha3, "legacy": legacy, "checksum": checksum}

if __name__ == "__main__":
    for name, f in TESTS.items():
        with open(os.path.join(here, name + ".out"), "w") as fh:
            fh.write("\n".join(f()) + "\n")
