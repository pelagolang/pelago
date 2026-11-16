"""Reference implementations of FNV, xxHash32/64 and SipHash written from the specifications (not the official
code); used to generate test expectations. Spot-checked against published values in gen_crypto.py."""
M32, M64 = 0xffffffff, (1 << 64) - 1

def rotl32(x, n): return ((x << n) | (x >> (32 - n))) & M32
def rotl64(x, n): return ((x << n) | (x >> (64 - n))) & M64

def fnv(data, bits, a):
    prime = {32: 0x01000193, 64: 0x00000100000001b3}[bits]
    h = {32: 0x811c9dc5, 64: 0xcbf29ce484222325}[bits]
    mask = (1 << bits) - 1
    for b in data:
        if a:
            h = ((h ^ b) * prime) & mask
        else:
            h = ((h * prime) & mask) ^ b
    return h

def xxh32(data, seed=0):
    P1, P2, P3, P4, P5 = 2654435761, 2246822519, 3266489917, 668265263, 374761393
    n, i = len(data), 0
    if n >= 16:
        v = [(seed + P1 + P2) & M32, (seed + P2) & M32, seed & M32, (seed - P1) & M32]
        while i + 16 <= n:
            for k in range(4):
                w = int.from_bytes(data[i + 4 * k:i + 4 * k + 4], "little")
                v[k] = (rotl32((v[k] + w * P2) & M32, 13) * P1) & M32
            i += 16
        h = (rotl32(v[0], 1) + rotl32(v[1], 7) + rotl32(v[2], 12) + rotl32(v[3], 18)) & M32
    else:
        h = (seed + P5) & M32
    h = (h + n) & M32
    while i + 4 <= n:
        h = (rotl32((h + int.from_bytes(data[i:i + 4], "little") * P3) & M32, 17) * P4) & M32
        i += 4
    while i < n:
        h = (rotl32((h + data[i] * P5) & M32, 11) * P1) & M32
        i += 1
    h ^= h >> 15; h = (h * P2) & M32; h ^= h >> 13; h = (h * P3) & M32; h ^= h >> 16
    return h

def xxh64(data, seed=0):
    P1, P2, P3, P4, P5 = 11400714785074694791, 14029467366897019727, 1609587929392839161, 9650029242287828579, 2870177450012600261
    rnd = lambda acc, x: (rotl64((acc + x * P2) & M64, 31) * P1) & M64
    n, i = len(data), 0
    if n >= 32:
        v = [(seed + P1 + P2) & M64, (seed + P2) & M64, seed & M64, (seed - P1) & M64]
        while i + 32 <= n:
            for k in range(4):
                v[k] = rnd(v[k], int.from_bytes(data[i + 8 * k:i + 8 * k + 8], "little"))
            i += 32
        h = (rotl64(v[0], 1) + rotl64(v[1], 7) + rotl64(v[2], 12) + rotl64(v[3], 18)) & M64
        for k in range(4):
            h = ((h ^ rnd(0, v[k])) * P1 + P4) & M64
    else:
        h = (seed + P5) & M64
    h = (h + n) & M64
    while i + 8 <= n:
        h ^= rnd(0, int.from_bytes(data[i:i + 8], "little"))
        h = (rotl64(h, 27) * P1 + P4) & M64
        i += 8
    if i + 4 <= n:
        h ^= (int.from_bytes(data[i:i + 4], "little") * P1) & M64
        h = (rotl64(h, 23) * P2 + P3) & M64
        i += 4
    while i < n:
        h ^= (data[i] * P5) & M64
        h = (rotl64(h, 11) * P1) & M64
        i += 1
    h ^= h >> 33; h = (h * P2) & M64; h ^= h >> 29; h = (h * P3) & M64; h ^= h >> 32
    return h

def siphash(key, data, c=2, d=4):
    k0, k1 = int.from_bytes(key[:8], "little"), int.from_bytes(key[8:], "little")
    v = [k0 ^ 0x736f6d6570736575, k1 ^ 0x646f72616e646f6d, k0 ^ 0x6c7967656e657261, k1 ^ 0x7465646279746573]
    def rnd():
        v[0] = (v[0] + v[1]) & M64; v[1] = rotl64(v[1], 13); v[1] ^= v[0]; v[0] = rotl64(v[0], 32)
        v[2] = (v[2] + v[3]) & M64; v[3] = rotl64(v[3], 16); v[3] ^= v[2]
        v[0] = (v[0] + v[3]) & M64; v[3] = rotl64(v[3], 21); v[3] ^= v[0]
        v[2] = (v[2] + v[1]) & M64; v[1] = rotl64(v[1], 17); v[1] ^= v[2]; v[2] = rotl64(v[2], 32)
    n = len(data)
    for i in range(0, n - n % 8, 8):
        m = int.from_bytes(data[i:i + 8], "little")
        v[3] ^= m
        for _ in range(c): rnd()
        v[0] ^= m
    last = int.from_bytes(data[n - n % 8:], "little") | ((n & 0xff) << 56)
    v[3] ^= last
    for _ in range(c): rnd()
    v[0] ^= last
    v[2] ^= 0xff
    for _ in range(d): rnd()
    return v[0] ^ v[1] ^ v[2] ^ v[3]
