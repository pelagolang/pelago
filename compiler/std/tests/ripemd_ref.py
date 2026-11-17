"""Independent RIPEMD-128/160/256/320 reference for gen_crypto.py (160 is checked against hashlib; the others against the published values for \"abc\" below)."""
import struct
M=0xffffffff
def rotl(x,n): return ((x<<n)|(x>>(32-n)))&M
R=[list(range(16)),[7,4,13,1,10,6,15,3,12,0,9,5,2,14,11,8],[3,10,14,4,9,15,8,1,2,7,0,6,13,11,5,12],[1,9,11,10,0,8,12,4,13,3,7,15,14,5,6,2],[4,0,5,9,7,12,2,10,14,1,3,8,11,6,15,13]]
RP=[[5,14,7,0,9,2,11,4,13,6,15,8,1,10,3,12],[6,11,3,7,0,13,5,10,14,15,8,12,4,9,1,2],[15,5,1,3,7,14,6,9,11,8,12,2,10,0,4,13],[8,6,4,1,3,11,15,0,5,12,2,13,9,7,10,14],[12,15,10,4,1,5,8,7,6,2,13,14,0,3,9,11]]
S=[[11,14,15,12,5,8,7,9,11,13,14,15,6,7,9,8],[7,6,8,13,11,9,7,15,7,12,15,9,11,7,13,12],[11,13,6,7,14,9,13,15,14,8,13,6,5,12,7,5],[11,12,14,15,14,15,9,8,9,14,5,6,8,6,5,12],[9,15,5,11,6,8,13,12,5,12,13,14,11,8,5,6]]
SP=[[8,9,9,11,13,15,15,5,7,7,8,11,14,14,12,6],[9,13,15,7,12,8,9,11,7,7,12,7,6,15,13,11],[9,7,15,11,8,6,6,14,12,13,5,14,13,13,7,5],[15,5,8,11,14,14,6,14,6,9,12,9,12,5,15,8],[8,5,12,9,12,5,14,6,8,13,6,5,15,13,11,11]]
K=[0,0x5A827999,0x6ED9EBA1,0x8F1BBCDC,0xA953FD4E]
KP=[0x50A28BE6,0x5C4DD124,0x6D703EF3,0x7A6D76E9,0]
K4=[0,0x5A827999,0x6ED9EBA1,0x8F1BBCDC]
KP4=[0x50A28BE6,0x5C4DD124,0x6D703EF3,0]
def f(r,x,y,z):
    if r==0: return x^y^z
    if r==1: return (x&y)|(~x&M&z)
    if r==2: return (x|(~y&M))^z
    if r==3: return (x&z)|(y&(~z&M))
    return x^(y|(~z&M))
def pad(m):
    n=len(m); m=m+b"\x80"
    while len(m)%64!=56: m+=b"\0"
    return m+struct.pack("<Q",n*8)
def rmd(m,bits):
    wide=bits in(256,320); five=bits in(160,320)
    if five: h=[0x67452301,0xefcdab89,0x98badcfe,0x10325476,0xc3d2e1f0]+([0x76543210,0xfedcba98,0x89abcdef,0x01234567,0x3c2d1e0f] if wide else [])
    else: h=[0x67452301,0xefcdab89,0x98badcfe,0x10325476]+([0x76543210,0xfedcba98,0x89abcdef,0x01234567] if wide else [])
    m=pad(m)
    for off in range(0,len(m),64):
        X=struct.unpack("<16I",m[off:off+64])
        n=5 if five else 4
        a=h[:n]; b=(h[n:] if wide else h[:n])[:]
        a=a[:]; 
        for rd in range(n):
            for k in range(16):
                if five:
                    t=(rotl((a[0]+f(rd,a[1],a[2],a[3])+X[R[rd][k]]+K[rd])&M,S[rd][k])+a[4])&M
                    a=[a[4],t,a[1],rotl(a[2],10),a[3]]
                    t=(rotl((b[0]+f(4-rd,b[1],b[2],b[3])+X[RP[rd][k]]+KP[rd])&M,SP[rd][k])+b[4])&M
                    b=[b[4],t,b[1],rotl(b[2],10),b[3]]
                else:
                    t=rotl((a[0]+f(rd,a[1],a[2],a[3])+X[R[rd][k]]+K4[rd])&M,S[rd][k])
                    a=[a[3],t,a[1],a[2]]
                    t=rotl((b[0]+f(3-rd,b[1],b[2],b[3])+X[RP[rd][k]]+KP4[rd])&M,SP[rd][k])
                    b=[b[3],t,b[1],b[2]]
            if wide:
                sw={160:None}
                if five: i=[1,3,0,2,4][rd]
                else: i=rd
                # 256: swap A,B,C,D by round; 320: B,D,A,C,E (positions in rotated order are canonical after 16 steps)
                a[i],b[i]=b[i],a[i]
        if not wide:
            if five:
                t=(h[1]+a[2]+b[3])&M; h[1]=(h[2]+a[3]+b[4])&M; h[2]=(h[3]+a[4]+b[0])&M; h[3]=(h[4]+a[0]+b[1])&M; h[4]=(h[0]+a[1]+b[2])&M; h[0]=t
            else:
                t=(h[1]+a[2]+b[3])&M; h[1]=(h[2]+a[3]+b[0])&M; h[2]=(h[3]+a[0]+b[1])&M; h[3]=(h[0]+a[1]+b[2])&M; h[0]=t
        else:
            for i in range(n):
                h[i]=(h[i]+a[i])&M; h[n+i]=(h[n+i]+b[i])&M
    return b"".join(struct.pack("<I",x) for x in h)
if __name__=="__main__":
    import hashlib
    for t in [b"",b"abc",b"a"*100,bytes(range(200))]:
        assert rmd(t,160)==hashlib.new("ripemd160",t).digest(), "160"
    print("160 ok")
    for bits,exp in [(128,"c14a12199c66e4ba84636b0f69144c77"),(256,"afbd6e228b9d8cbbcef5ca2d03e6dba10ac0bc7dcbe4680e1e42d2e975459b65"),(320,"de4c01b3054f8930a79d09ae738e92301e5a17085beffdc1b8d116713e74f82fa942d64cdbc4682d")]:
        print(bits, rmd(b"abc",bits).hex()==exp, rmd(b"abc",bits).hex())
