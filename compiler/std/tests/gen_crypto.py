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
    # SP 800-185, from the text of the standard (independent of the Pelago code)
    def enc(x, left):
        b = x.to_bytes(max(1, (x.bit_length() + 7) // 8), "big")
        return bytes([len(b)]) + b if left else b + bytes([len(b)])
    es = lambda s: enc(len(s) * 8, True) + s
    def bytepad(x, w):
        z = enc(w, True) + x
        return z + bytes(-len(z) % w)
    def cshake(strength, data, n, name, custom):
        rate = 200 - strength // 4
        if not name and not custom:
            return sponge(data, rate, 31, n)
        return sponge(bytepad(es(name) + es(custom), rate) + data, rate, 4, n)
    def kmac(strength, key, data, n, custom, xof=False):
        rate = 200 - strength // 4
        msg = bytepad(es(key), rate) + data + enc(0 if xof else n * 8, False)
        return cshake(strength, msg, n, b"KMAC", custom)
    email = b"Email Signature"
    assert cshake(128, bytes([0, 1, 2, 3]), 32, b"", email).hex() == "c1c36925b6409a04f1b504fcbca9d82b4017277cb5ed2b2065fc1d3814d5aaf5"
    key = bytes(range(0x40, 0x60))
    assert kmac(128, key, bytes([0, 1, 2, 3]), 32, b"").hex() == "e5780b0d3ea6f7d3a429c5706aa43a00fadbd7d49628839e3187243f456ee14e"
    out.append(cshake(128, pattern(4), 32, b"", email).hex())
    out.append(cshake(256, pattern(200), 64, b"name", email).hex())
    out.append(cshake(128, pattern(7), 16, b"", b"").hex())
    out.append(kmac(128, key, bytes([0, 1, 2, 3]), 32, b"").hex())
    out.append(kmac(256, key, pattern(300), 64, email).hex())
    out.append(kmac(128, b"", pattern(10), 48, b"").hex())
    out.append(kmac(256, key, pattern(33), 16, email, True).hex())
    out.append(kmac(256, key, pattern(33), 150, email, True).hex())
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

def blake2():
    out = []
    for n in [0, 1, 63, 64, 65, 127, 128, 129, 255, 256, 257, 300]:
        m = pattern(n)
        out.append(f"{n} {hashlib.blake2b(m).hexdigest()} {hashlib.blake2s(m).hexdigest()}")
    for size in [1, 20, 32, 48]:
        m = pattern(100)
        out.append(f"{size} {hashlib.blake2b(m, digest_size=size).hexdigest()} {hashlib.blake2s(m, digest_size=min(size, 32)).hexdigest()}")
    for kl in [1, 7, 32]:
        for n in [0, 5, 64, 128, 129]:
            m, k = pattern(n), pattern(kl)
            out.append(f"{kl} {n} {hashlib.blake2b(m, digest_size=32, key=k).hexdigest()} {hashlib.blake2s(m, digest_size=32, key=k).hexdigest()}")
    out.append(hashlib.blake2b(pattern(200), digest_size=64, key=pattern(64)).hexdigest())
    out.append(hashlib.blake2b(pattern(77), digest_size=40, key=pattern(9), salt=pattern(16), person=pattern(16)).hexdigest())
    out.append(hashlib.blake2s(pattern(77), digest_size=24, key=pattern(9), salt=pattern(8), person=pattern(8)).hexdigest())
    out.append("true")
    return out

def blake3():
    import blake3_ref as b3ref
    pat = lambda n: bytes(i % 251 for i in range(n))
    assert b3ref.blake3(b"").hex() == "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    assert b3ref.blake3(b"\0").hex() == "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"
    assert b3ref.blake3(b"abc").hex() == "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
    out = []
    for n in [0, 1, 2, 3, 63, 64, 65, 127, 128, 129, 1023, 1024, 1025, 2048, 2049, 3072, 3073, 4096, 4097]:
        out.append(f"{n} {b3ref.blake3(pat(n)).hex()}")
    for n in [0, 1, 64, 1024, 1025, 3073]:
        out.append(f"{n} {b3ref.keyed(pat(32), pat(n)).hex()}")
    for n in [0, 5, 1025]:
        out.append(f"{n} {b3ref.derive_key(b'blake3 2019-12-27 16:29:52 test vectors context', pat(n)).hex()}")
    out.append(b3ref.blake3(pat(1500), outlen=32).hex())
    out.append(b3ref.blake3(pat(1500), outlen=200).hex())
    out.append(b3ref.keyed(pat(32), pat(70), 100).hex())
    out.append("true")
    return out

def fasthash():
    import fast_ref as f
    out = []
    pat = lambda n: pattern(n)
    for n in [0, 1, 3, 4, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 100, 1000]:
        m = pat(n)
        out.append(f"{n} {f.fnv(m, 32, True):08x} {f.fnv(m, 64, True):016x} {f.fnv(m, 32, False):08x} {f.fnv(m, 64, False):016x}")
        out.append(f"{n} {f.xxh32(m, 0):08x} {f.xxh32(m, 12345):08x} {f.xxh64(m, 0):016x} {f.xxh64(m, 12345):016x}")
    spam = b"Nobody inspects the spammish repetition"
    assert f.xxh32(spam) == 0xe2293b2f and f.xxh64(spam) == 0xfbcea83c8a378bf1
    assert f.xxh32(b"") == 0x02cc5d05 and f.xxh64(b"") == 0xef46db3751d8e999
    out.append(f"{f.xxh32(spam):08x} {f.xxh64(spam):016x}")
    out.append(f"{f.xxh32(b''):08x} {f.xxh64(b''):016x}")
    key = bytes(range(16))
    assert f.siphash(key, bytes(range(15))) == 0xa129ca6149be45e5 and f.siphash(key, b"") == 0x726fdb47dd0e0e31
    sip = lambda k, m, c, d: f.siphash(k, m, c, d).to_bytes(8, "little").hex()
    for n in [0, 1, 7, 8, 9, 15, 16, 17, 40]:
        m = bytes(range(n))
        out.append(f"{n} {sip(key, m, 2, 4)} {sip(key, m, 1, 3)}")
    out.append("true")
    return out

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from cryptography.hazmat.primitives import poly1305 as _poly

def cc_xor(key, nonce, counter, data):
    n16 = counter.to_bytes(4, "little") + nonce
    return Cipher(algorithms.ChaCha20(key, n16), mode=None).encryptor().update(data)

def cc_block(key, nonce, counter):
    return cc_xor(key, nonce, counter, bytes(64))

def rotl(x, n): return ((x << n) | (x >> (32 - n))) & 0xffffffff

def hchacha(key, n16):
    import struct
    x = list(struct.unpack("<4I", b"expand 32-byte k")) + list(struct.unpack("<8I", key)) + list(struct.unpack("<4I", n16))
    def qr(a, b, c, d):
        x[a] = (x[a] + x[b]) & 0xffffffff; x[d] = rotl(x[d] ^ x[a], 16)
        x[c] = (x[c] + x[d]) & 0xffffffff; x[b] = rotl(x[b] ^ x[c], 12)
        x[a] = (x[a] + x[b]) & 0xffffffff; x[d] = rotl(x[d] ^ x[a], 8)
        x[c] = (x[c] + x[d]) & 0xffffffff; x[b] = rotl(x[b] ^ x[c], 7)
    for _ in range(10):
        qr(0, 4, 8, 12); qr(1, 5, 9, 13); qr(2, 6, 10, 14); qr(3, 7, 11, 15)
        qr(0, 5, 10, 15); qr(1, 6, 11, 12); qr(2, 7, 8, 13); qr(3, 4, 9, 14)
    return struct.pack("<8I", *(x[0:4] + x[12:16]))

import struct
# the draft's published HChaCha20 vector guards the reference above
assert hchacha(bytes(range(32)), bytes.fromhex("000000090000004a0000000031415927")).hex() == \
    "82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc"

def key32(k): return bytes((i * k + 5) & 255 for i in range(32))

def chacha():
    out = []
    for kk in [1, 3, 9]:
        key = key32(kk)
        for nn in [0, 7]:
            nonce = bytes((i * 11 + nn) & 255 for i in range(12))
            for ctr in [0, 1, 255, 4294967295]:
                out.append(f"{kk} {nn} {ctr} {cc_block(key, nonce, ctr).hex()}")
    key = key32(2); nonce = bytes(range(12))
    for n in [0, 1, 63, 64, 65, 127, 128, 129, 300]:
        m = pattern(n)
        out.append(f"{n} {cc_xor(key, nonce, 1, m).hex()}")
    out.append("true")
    n24 = bytes(range(24))
    for n in [0, 5, 64, 100]:
        m = pattern(n)
        sub = hchacha(key, n24[:16])
        out.append(f"{n} {cc_xor(sub, bytes(4) + n24[16:], 1, m).hex()}")
    out.append(hchacha(bytes(range(32)), bytes.fromhex("000000090000004a0000000031415927")).hex())
    return out

def poly1305():
    out = []
    for kk in [1, 7, 13]:
        key = key32(kk)
        for n in list(range(0, 36)) + [47, 48, 49, 100, 255, 256, 1000]:
            m = pattern(n)
            out.append(f"{kk} {n} {_poly.Poly1305.generate_tag(key, m).hex()}")
    # edge keys: r clamps, all-ones accumulator patterns
    for key in [bytes([255] * 32), bytes(32), bytes([0xff] * 16 + [0] * 16)]:
        for m in [b"", b"\xff" * 16, b"\xff" * 17, b"\xff" * 100, bytes(16)]:
            out.append(f"{key[0]} {len(m)} {_poly.Poly1305.generate_tag(key, m).hex()}")
    out.append(_poly.Poly1305.generate_tag(bytes.fromhex("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b"), b"Cryptographic Forum Research Group").hex())
    out.append(out[-1])
    out.append("true false")
    return out

def aead():
    out = []
    for kk in [1, 5]:
        key = key32(kk)
        nonce = bytes((i * 3 + kk) & 255 for i in range(12))
        for n in [0, 1, 15, 16, 17, 63, 64, 65, 130]:
            for al in [0, 1, 16, 20]:
                m, a = pattern(n), pat(al, 9)
                out.append(f"{kk} {n} {al} {ChaCha20Poly1305(key).encrypt(nonce, m, a or None).hex()}")
    key = key32(4); n24 = bytes((i * 5 + 1) & 255 for i in range(24))
    sub = hchacha(key, n24[:16]); n12 = bytes(4) + n24[16:]
    for n in [0, 1, 17, 100]:
        for al in [0, 12]:
            m, a = pattern(n), pat(al, 9)
            out.append(f"{n} {al} {ChaCha20Poly1305(sub).encrypt(n12, m, a or None).hex()}")
    out += ["true"] + ["true"] * 7 + ["16 true"]
    return out

def csprng():
    out = []
    for kk in [1, 2]:
        key = key32(kk); count = 0; buf = b""; stream = b""
        while len(stream) < 200:
            b = cc_block(key, bytes(12), count); count += 1
            key, buf = b[:32], b[32:]
            stream += buf
        out.append(stream[:200].hex())
    out += ["true", "true true", "true", "true"]
    return out

from cryptography.hazmat.primitives.kdf.argon2 import Argon2d, Argon2i, Argon2id

def password():
    out = []
    out.append(hashlib.scrypt(b"password", salt=b"NaCl", n=16, r=1, p=1, dklen=64).hex())
    out.append(hashlib.scrypt(b"", salt=b"", n=16, r=1, p=1, dklen=64).hex())
    out.append(hashlib.scrypt(b"pleaseletmein", salt=b"SodiumChloride", n=64, r=2, p=1, dklen=32).hex())
    out.append(hashlib.scrypt(b"pw", salt=b"salt", n=8, r=1, p=2, dklen=20).hex())
    cls = [Argon2d, Argon2i, Argon2id]
    def a(k, pw, salt, t, m, l, n, secret=None, ad=None):
        return cls[k](salt=salt, length=n, iterations=t, lanes=l, memory_cost=m, secret=secret, ad=ad).derive(pw).hex()
    for k in range(3):
        out.append(f"{k} {a(k, b'password', b'saltsalt', 2, 16, 2, 32)}")
        out.append(f"{k} {a(k, b'password', b'saltsalt', 1, 8, 1, 16)}")
    return out

def password_more():
    out = []
    cls = [Argon2d, Argon2i, Argon2id]
    def a(k, pw, salt, t, m, l, n, secret=None, ad=None):
        return cls[k](salt=salt, length=n, iterations=t, lanes=l, memory_cost=m, secret=secret, ad=ad).derive(pw).hex()
    out.append(a(2, b"password", b"saltsalt", 2, 16, 2, 32, b"sec", b"ad"))
    out.append(a(2, b"pw", b"somesalt", 1, 16, 2, 100))
    for k in (2, 1, 0):
        out.append(a(k, b"password", b"saltsalt", 1, 16, 1, 40))
    return out

TESTS = {"password": password, "password_more": password_more, "chacha": chacha, "poly1305": poly1305, "aead": aead, "csprng": csprng, "fasthash": fasthash, "blake3": blake3, "blake2": blake2, "sha2": sha2, "mac": mac, "sha3": sha3, "legacy": legacy, "checksum": checksum}

if __name__ == "__main__":
    for name, f in TESTS.items():
        with open(os.path.join(here, name + ".out"), "w") as fh:
            fh.write("\n".join(f()) + "\n")
