//! Ed25519 signature verification (RFC 8032), verify only, no dependencies.
//!
//! It runs once per update on a few hundred bytes, so it favours being short and plain over being
//! fast: field elements are four 64-bit limbs modulo 2^255 - 19, points use extended coordinates and
//! the scalar multiplication is a bit-by-bit double and add. It is not constant time and must never
//! be used with secret keys. Checked against OpenSSL and the RFC 8032 vectors in the tests.

use super::sha2::sha512;

type Limbs = [u64; 4];

const P: Limbs = [
    0xffff_ffff_ffff_ffed,
    u64::MAX,
    u64::MAX,
    0x7fff_ffff_ffff_ffff,
];
/// The group order 2^252 + 27742317777372353535851937790883648493.
const L: Limbs = [
    0x5812_631a_5cf5_d3ed,
    0x14de_f9de_a2f7_9cd6,
    0,
    0x1000_0000_0000_0000,
];

fn ge(a: &Limbs, b: &Limbs) -> bool {
    for i in (0..4).rev() {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    true
}

fn sub_limbs(a: &Limbs, b: &Limbs) -> (Limbs, bool) {
    let mut out = [0u64; 4];
    let mut borrow = false;
    for i in 0..4 {
        let (d1, b1) = a[i].overflowing_sub(b[i]);
        let (d2, b2) = d1.overflowing_sub(borrow as u64);
        out[i] = d2;
        borrow = b1 || b2;
    }
    (out, borrow)
}

fn add_limbs(a: &Limbs, b: &Limbs) -> (Limbs, bool) {
    let mut out = [0u64; 4];
    let mut carry = false;
    for i in 0..4 {
        let (s1, c1) = a[i].overflowing_add(b[i]);
        let (s2, c2) = s1.overflowing_add(carry as u64);
        out[i] = s2;
        carry = c1 || c2;
    }
    (out, carry)
}

/// A field element, always fully reduced (below P).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Fe(Limbs);

impl Fe {
    const ZERO: Fe = Fe([0; 4]);
    const ONE: Fe = Fe([1, 0, 0, 0]);

    fn small(v: u64) -> Fe {
        Fe([v, 0, 0, 0])
    }

    fn from_bytes(b: &[u8; 32]) -> Fe {
        let mut l = [0u64; 4];
        for i in 0..4 {
            let mut w = [0u8; 8];
            w.copy_from_slice(&b[8 * i..8 * i + 8]);
            l[i] = u64::from_le_bytes(w);
        }
        l[3] &= 0x7fff_ffff_ffff_ffff;
        while ge(&l, &P) {
            l = sub_limbs(&l, &P).0;
        }
        Fe(l)
    }

    fn to_bytes(self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..4 {
            out[8 * i..8 * i + 8].copy_from_slice(&self.0[i].to_le_bytes());
        }
        out
    }

    fn add(self, o: Fe) -> Fe {
        let (mut s, _) = add_limbs(&self.0, &o.0);
        if ge(&s, &P) {
            s = sub_limbs(&s, &P).0;
        }
        Fe(s)
    }

    fn sub(self, o: Fe) -> Fe {
        let (d, borrow) = sub_limbs(&self.0, &o.0);
        if borrow {
            Fe(add_limbs(&d, &P).0)
        } else {
            Fe(d)
        }
    }

    fn neg(self) -> Fe {
        Fe::ZERO.sub(self)
    }

    fn mul(self, o: Fe) -> Fe {
        let mut t = [0u64; 8];
        for i in 0..4 {
            let mut carry: u128 = 0;
            for j in 0..4 {
                let cur = t[i + j] as u128 + (self.0[i] as u128) * (o.0[j] as u128) + carry;
                t[i + j] = cur as u64;
                carry = cur >> 64;
            }
            t[i + 4] = carry as u64;
        }
        // 2^256 = 38 (mod P): fold the high half into the low half, twice, then reduce.
        let mut r = [0u64; 5];
        let mut carry: u128 = 0;
        for i in 0..4 {
            let cur = t[i] as u128 + (t[i + 4] as u128) * 38 + carry;
            r[i] = cur as u64;
            carry = cur >> 64;
        }
        r[4] = carry as u64;
        let mut carry = (r[4] as u128) * 38;
        let mut low = [r[0], r[1], r[2], r[3]];
        for limb in low.iter_mut() {
            let cur = *limb as u128 + carry;
            *limb = cur as u64;
            carry = cur >> 64;
        }
        if carry > 0 {
            let mut c: u128 = 38;
            for limb in low.iter_mut() {
                let cur = *limb as u128 + c;
                *limb = cur as u64;
                c = cur >> 64;
            }
        }
        while ge(&low, &P) {
            low = sub_limbs(&low, &P).0;
        }
        Fe(low)
    }

    fn pow(self, exp: &Limbs) -> Fe {
        let mut result = Fe::ONE;
        for i in (0..256).rev() {
            result = result.mul(result);
            if (exp[i / 64] >> (i % 64)) & 1 == 1 {
                result = result.mul(self);
            }
        }
        result
    }

    fn inv(self) -> Fe {
        // P - 2
        self.pow(&[
            0xffff_ffff_ffff_ffeb,
            u64::MAX,
            u64::MAX,
            0x7fff_ffff_ffff_ffff,
        ])
    }
}

/// d = -121665 / 121666
fn curve_d() -> Fe {
    Fe::small(121665).neg().mul(Fe::small(121666).inv())
}

#[derive(Clone, Copy)]
struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl Point {
    const IDENTITY: Point = Point {
        x: Fe::ZERO,
        y: Fe::ONE,
        z: Fe::ONE,
        t: Fe::ZERO,
    };

    fn decompress(bytes: &[u8; 32]) -> Option<Point> {
        let sign = bytes[31] >> 7;
        let mut yb = *bytes;
        yb[31] &= 0x7f;
        let y = Fe::from_bytes(&yb);
        if y.to_bytes() != yb {
            return None; // not canonical
        }
        let y2 = y.mul(y);
        let u = y2.sub(Fe::ONE);
        let v = curve_d().mul(y2).add(Fe::ONE);
        // x = (u/v)^((P+3)/8), corrected by sqrt(-1) when needed.
        let ratio = u.mul(v.inv());
        let mut x = ratio.pow(&[
            0xffff_ffff_ffff_fffe,
            u64::MAX,
            u64::MAX,
            0x0fff_ffff_ffff_ffff,
        ]);
        if x.mul(x) != ratio {
            let i = Fe::small(2).pow(&[
                0xffff_ffff_ffff_fffb,
                u64::MAX,
                u64::MAX,
                0x1fff_ffff_ffff_ffff,
            ]);
            x = x.mul(i);
            if x.mul(x) != ratio {
                return None;
            }
        }
        if x == Fe::ZERO && sign == 1 {
            return None;
        }
        if (x.0[0] & 1) as u8 != sign {
            x = x.neg();
        }
        Some(Point {
            x,
            y,
            z: Fe::ONE,
            t: x.mul(y),
        })
    }

    fn encode(&self) -> [u8; 32] {
        let zi = self.z.inv();
        let x = self.x.mul(zi);
        let y = self.y.mul(zi);
        let mut out = y.to_bytes();
        out[31] |= ((x.0[0] & 1) as u8) << 7;
        out
    }

    /// The unified addition for a = -1 twisted Edwards curves (also doubles).
    fn add(&self, o: &Point) -> Point {
        let d2 = curve_d().add(curve_d());
        let a = self.y.sub(self.x).mul(o.y.sub(o.x));
        let b = self.y.add(self.x).mul(o.y.add(o.x));
        let c = self.t.mul(d2).mul(o.t);
        let d = self.z.add(self.z).mul(o.z);
        let e = b.sub(a);
        let f = d.sub(c);
        let g = d.add(c);
        let h = b.add(a);
        Point {
            x: e.mul(f),
            y: g.mul(h),
            z: f.mul(g),
            t: e.mul(h),
        }
    }

    fn mul(&self, scalar: &Limbs) -> Point {
        let mut acc = Point::IDENTITY;
        for i in (0..256).rev() {
            acc = acc.add(&acc);
            if (scalar[i / 64] >> (i % 64)) & 1 == 1 {
                acc = acc.add(self);
            }
        }
        acc
    }
}

fn limbs_from_le(b: &[u8; 32]) -> Limbs {
    let mut l = [0u64; 4];
    for i in 0..4 {
        let mut w = [0u8; 8];
        w.copy_from_slice(&b[8 * i..8 * i + 8]);
        l[i] = u64::from_le_bytes(w);
    }
    l
}

/// A 512-bit little-endian number modulo L, one bit at a time.
fn reduce_mod_l(h: &[u8; 64]) -> Limbs {
    let mut r: Limbs = [0; 4];
    for byte in h.iter().rev() {
        for bit in (0..8).rev() {
            let (mut d, _) = add_limbs(&r, &r);
            d[0] |= ((byte >> bit) & 1) as u64;
            if ge(&d, &L) {
                d = sub_limbs(&d, &L).0;
            }
            r = d;
        }
    }
    r
}

/// True when `sig` is a valid Ed25519 signature of `msg` under the public key `pk`.
pub fn verify(pk: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    let mut rb = [0u8; 32];
    rb.copy_from_slice(&sig[..32]);
    let mut sb = [0u8; 32];
    sb.copy_from_slice(&sig[32..]);
    let s = limbs_from_le(&sb);
    if ge(&s, &L) {
        return false;
    }
    let (Some(a), Some(r)) = (Point::decompress(pk), Point::decompress(&rb)) else {
        return false;
    };
    let mut data = Vec::with_capacity(64 + msg.len());
    data.extend_from_slice(&rb);
    data.extend_from_slice(pk);
    data.extend_from_slice(msg);
    let k = reduce_mod_l(&sha512(&data));
    let Some(base) = Point::decompress(&{
        let mut b = [0x66u8; 32];
        b[0] = 0x58;
        b
    }) else {
        return false;
    };
    let left = base.mul(&s);
    let right = r.add(&a.mul(&k));
    left.encode() == right.encode()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut out = [0u8; N];
        for i in 0..N {
            out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn rfc8032_test_vector_1() {
        let pk = unhex::<32>("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let sig = unhex::<64>(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        );
        assert!(verify(&pk, b"", &sig));
        assert!(!verify(&pk, b"x", &sig));
    }

    #[test]
    fn openssl_signature() {
        let pk = unhex::<32>("8027b65c6bf578f5451067a2e51cab0090d695d5e1b254813d39fa0b9763e20c");
        let sig = unhex::<64>(
            "2de9c7c8ffc74291978579f33d1ca96fb82d0d564c3360f50934a2befdc67319036019ea86e766f2ffa2b07833b115bddcdb66d57741463143523d4ed40ecf0a",
        );
        let msg = b"abc123  pelago-0.1.0-x86_64-linux.tar.gz\n";
        assert!(verify(&pk, msg, &sig));
        let mut bad = *msg;
        bad[0] = b'x';
        assert!(!verify(&pk, &bad, &sig));
        let mut flipped = sig;
        flipped[10] ^= 1;
        assert!(!verify(&pk, msg, &flipped));
        let mut other = pk;
        other[0] ^= 1;
        assert!(!verify(&other, msg, &sig));
    }

    #[test]
    fn rejects_a_scalar_above_the_group_order() {
        let pk = unhex::<32>("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let mut sig = unhex::<64>(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        );
        sig[63] = 0xff;
        assert!(!verify(&pk, b"", &sig));
    }
}
