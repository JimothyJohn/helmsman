//! MD5 (RFC 1321). The Moxa console hashes its password-change fields with
//! it in the page's script, so a client has to as well. Not for anything
//! else: MD5 is broken as a security hash.

// RFC 1321 names its variables a, b, c, d, f, g, k, m; the code keeps them.
#![allow(clippy::many_single_char_names, clippy::chunks_exact_to_as_chunks)]

const SHIFT: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5,
    9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6,
    10, 15, 21, 6, 10, 15, 21,
];

/// RFC 1321's `T[i] = floor(2^32 * abs(sin(i + 1)))`, as the RFC defines it.
/// The RFC's own test suite (tests below) pins the result.
fn k_table() -> [u32; 64] {
    let mut k = [0u32; 64];
    for (i, slot) in k.iter_mut().enumerate() {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        {
            *slot = (((i + 1) as f64).sin().abs() * 4_294_967_296.0) as u32;
        }
    }
    k
}

/// The MD5 digest of `data`.
#[must_use]
pub fn md5(data: &[u8]) -> [u8; 16] {
    let k = k_table();
    let (mut a0, mut b0, mut c0, mut d0) = (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (w, b) in m.iter_mut().zip(chunk.chunks_exact(4)) {
            *w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(SHIFT[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (dst, v) in out.chunks_exact_mut(4).zip([a0, b0, c0, d0]) {
        dst.copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// Lowercase hex of `bytes`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 1321 section A.5.
    #[test]
    fn rfc1321_test_suite() {
        let cases: [(&str, &str); 7] = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(hex(&md5(input.as_bytes())), want, "md5({input:?})");
        }
    }

    /// The padding boundaries (55, 56, 63, 64 and 65 bytes of `a`), digests
    /// from Python's hashlib.
    #[test]
    fn padding_boundaries() {
        let cases = [
            (55, "ef1772b6dff9a122358552954ad0df65"),
            (56, "3b0c8ac703f828b04c6c197006d17218"),
            (63, "b06521f39153d618550606be297466d5"),
            (64, "014842d480b571495a4a0363793f7367"),
            (65, "c743a45e0d2e6a95cb859adae0248435"),
        ];
        for (n, want) in cases {
            assert_eq!(hex(&md5(&vec![b'a'; n])), want, "{n} bytes");
        }
    }
}
