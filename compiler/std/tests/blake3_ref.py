"""Reference BLAKE3 written from the specification (not the official code); used to generate test expectations."""
IV = [0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19]
PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8]
CS, CE, PA, RO, KH, DKC, DKM = 1, 2, 4, 8, 16, 32, 64
M32 = 0xffffffff
def rotr(x, n): return ((x >> n) | (x << (32 - n))) & M32
def g(v, a, b, c, d, x, y):
    v[a] = (v[a] + v[b] + x) & M32; v[d] = rotr(v[d] ^ v[a], 16)
    v[c] = (v[c] + v[d]) & M32; v[b] = rotr(v[b] ^ v[c], 12)
    v[a] = (v[a] + v[b] + y) & M32; v[d] = rotr(v[d] ^ v[a], 8)
    v[c] = (v[c] + v[d]) & M32; v[b] = rotr(v[b] ^ v[c], 7)
def compress(cv, m, counter, blen, flags):
    v = list(cv) + IV[:4] + [counter & M32, (counter >> 32) & M32, blen, flags]
    m = list(m)
    for r in range(7):
        g(v, 0, 4, 8, 12, m[0], m[1]); g(v, 1, 5, 9, 13, m[2], m[3]); g(v, 2, 6, 10, 14, m[4], m[5]); g(v, 3, 7, 11, 15, m[6], m[7])
        g(v, 0, 5, 10, 15, m[8], m[9]); g(v, 1, 6, 11, 12, m[10], m[11]); g(v, 2, 7, 8, 13, m[12], m[13]); g(v, 3, 4, 9, 14, m[14], m[15])
        if r < 6:
            m = [m[i] for i in PERM]
    for i in range(8):
        v[i] ^= v[i + 8]; v[i + 8] ^= cv[i]
    return v
def words(b):
    b = b + bytes(64 - len(b))
    return [int.from_bytes(b[4 * i:4 * i + 4], "little") for i in range(16)]
def chunk_output(key, chunk, counter, flags):
    cv = list(key)
    blocks = [chunk[i:i + 64] for i in range(0, len(chunk), 64)] or [b""]
    for i, blk in enumerate(blocks[:-1]):
        cv = compress(cv, words(blk), counter, 64, flags | (CS if i == 0 else 0))[:8]
    last = blocks[-1]
    return (cv, words(last), counter, len(last), flags | (CS if len(blocks) == 1 else 0) | CE)
def out_cv(o): return compress(o[0], o[1], o[2], o[3], o[4])[:8]
def parent(key, l, r, flags): return (list(key), l + r, 0, 64, flags | PA)
def blake3(data, key=IV, flags=0, outlen=32):
    chunks = [data[i:i + 1024] for i in range(0, len(data), 1024)] or [b""]
    stack = []
    for n, ch in enumerate(chunks[:-1]):
        cv = out_cv(chunk_output(key, ch, n, flags))
        total = n + 1
        while total & 1 == 0:
            cv = out_cv(parent(key, stack.pop(), cv, flags)); total >>= 1
        stack.append(cv)
    o = chunk_output(key, chunks[-1], len(chunks) - 1, flags)
    while stack:
        o = parent(key, stack.pop(), out_cv(o), flags)
    out = b""
    i = 0
    while len(out) < outlen:
        w = compress(o[0], o[1], i, o[3], o[4] | RO)
        out += b"".join(x.to_bytes(4, "little") for x in w)
        i += 1
    return out[:outlen]
def keyed(key32, data, outlen=32):
    return blake3(data, [int.from_bytes(key32[4 * i:4 * i + 4], "little") for i in range(8)], KH, outlen)
def derive_key(context, material, outlen=32):
    ck = blake3(context, IV, DKC, 32)
    return blake3(material, [int.from_bytes(ck[4 * i:4 * i + 4], "little") for i in range(8)], DKM, outlen)
if __name__ == "__main__":
    print(blake3(b"").hex())
    print(blake3(bytes([0])).hex())
    print(blake3(b"abc").hex())
