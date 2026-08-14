// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify

//! 手写 ChaCha20 流密码（RFC 8439），零新依赖。
//!
//! 用途：打包时加密内嵌网页 zip，运行时在内存中解密。定位是 **提高门槛的遮蔽**，
//! 不是不可破解的安全——密钥随每个 exe 内嵌，加密格式与本仓库一并公开。
//!
//! 加解密对称：`apply_keystream` 既是加密也是解密（流密码 XOR 特性）。

/// key/nonce 存进 config 前 XOR 的编译期遮蔽常量（诚实：开源即公开，仅挡 `strings`）。
const MASK: [u8; 32] = [
    0x9e, 0x37, 0x79, 0xb9, 0x7f, 0x4a, 0x7c, 0x15, 0xf3, 0x9c, 0xc0, 0x60, 0x5c, 0xed, 0xc8, 0x34,
    0x10, 0x82, 0x27, 0x6b, 0xf3, 0xa2, 0x72, 0x51, 0xf8, 0x6c, 0x6a, 0x11, 0xd0, 0xc1, 0x8e, 0x95,
];

/// ChaCha20 常量 "expand 32-byte k"（RFC 8439 §2.3）。
const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

#[inline]
fn rotl(v: u32, c: u32) -> u32 {
    v.rotate_left(c)
}

/// 单个 quarter round（RFC 8439 §2.1），就地作用于状态 4 个字。
#[inline]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = rotl(state[d] ^ state[a], 16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = rotl(state[b] ^ state[c], 12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = rotl(state[d] ^ state[a], 8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = rotl(state[b] ^ state[c], 7);
}

/// 由 key/nonce/counter 构造初始状态（RFC 8439 §2.3）。
fn init_state(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u32; 16] {
    let mut s = [0u32; 16];
    s[0..4].copy_from_slice(&CONSTANTS);
    for i in 0..8 {
        s[4 + i] = u32::from_le_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
    }
    s[12] = counter;
    for i in 0..3 {
        s[13 + i] = u32::from_le_bytes([
            nonce[i * 4],
            nonce[i * 4 + 1],
            nonce[i * 4 + 2],
            nonce[i * 4 + 3],
        ]);
    }
    s
}

/// 生成一个 64 字节 keystream block（RFC 8439 §2.3.1：20 轮 = 10×双轮）。
fn block(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    let initial = init_state(key, nonce, counter);
    let mut s = initial;
    for _ in 0..10 {
        // 列轮
        quarter_round(&mut s, 0, 4, 8, 12);
        quarter_round(&mut s, 1, 5, 9, 13);
        quarter_round(&mut s, 2, 6, 10, 14);
        quarter_round(&mut s, 3, 7, 11, 15);
        // 对角轮
        quarter_round(&mut s, 0, 5, 10, 15);
        quarter_round(&mut s, 1, 6, 11, 12);
        quarter_round(&mut s, 2, 7, 8, 13);
        quarter_round(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        let word = s[i].wrapping_add(initial[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// 对 `data` 就地施加 ChaCha20 keystream（加密与解密同一操作）。
/// 计数器从 1 开始（RFC 8439 §2.4：block 0 起始 counter=1）。
pub fn apply_keystream(key: &[u8; 32], nonce: &[u8; 12], data: &mut [u8]) {
    let mut counter: u32 = 1;
    for chunk in data.chunks_mut(64) {
        let ks = block(key, nonce, counter);
        for (b, k) in chunk.iter_mut().zip(ks.iter()) {
            *b ^= *k;
        }
        counter = counter.wrapping_add(1);
    }
}

/// 加密：返回新的密文向量（不改动入参）。
pub fn encrypt(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
    let mut buf = plaintext.to_vec();
    apply_keystream(key, nonce, &mut buf);
    buf
}

/// 解密：与 [`encrypt`] 对称。
pub fn decrypt(key: &[u8; 32], nonce: &[u8; 12], ciphertext: &[u8]) -> Vec<u8> {
    encrypt(key, nonce, ciphertext)
}

/// 用 OS 熵拼出随机字节（obfuscation 级，非 CSPRNG）。
///
/// 零依赖方案：`RandomState::new()` 每次取 OS 熵作 SipHash 种子，
/// 对不同输入哈希得到难以预测的 8 字节，多次拼接填满所需长度。
fn random_bytes(len: usize) -> Vec<u8> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut out = Vec::with_capacity(len);
    let mut counter: u64 = 0;
    while out.len() < len {
        // 每轮新建 RandomState -> 新的 OS 熵种子。
        let state = RandomState::new();
        let mut hasher = state.build_hasher();
        counter.hash(&mut hasher);
        // 再混入高精度时间与地址，增加每轮差异。
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        nanos.hash(&mut hasher);
        (&counter as *const u64 as usize).hash(&mut hasher);
        let h = hasher.finish();
        out.extend_from_slice(&h.to_le_bytes());
        counter = counter.wrapping_add(1);
    }
    out.truncate(len);
    out
}

/// 生成随机 32 字节 key。
pub fn random_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    k.copy_from_slice(&random_bytes(32));
    k
}

/// 生成随机 12 字节 nonce。
pub fn random_nonce() -> [u8; 12] {
    let mut n = [0u8; 12];
    n.copy_from_slice(&random_bytes(12));
    n
}

/// 遮蔽 32 字节 key：逐字节 XOR MASK，再 base64。诚实：仅轻度遮蔽。
pub fn mask_key_b64(key: &[u8; 32]) -> String {
    let mut m = [0u8; 32];
    for i in 0..32 {
        m[i] = key[i] ^ MASK[i];
    }
    crate::b64::encode(&m)
}

/// 遮蔽 12 字节 nonce：复用 MASK 前 12 字节。
pub fn mask_nonce_b64(nonce: &[u8; 12]) -> String {
    let mut m = [0u8; 12];
    for i in 0..12 {
        m[i] = nonce[i] ^ MASK[i];
    }
    crate::b64::encode(&m)
}

/// 去遮蔽还原 32 字节 key；base64 解码或长度不符时返回 None。
pub fn unmask_key(b64: &str) -> Option<[u8; 32]> {
    let bytes = crate::b64::decode(b64)?;
    if bytes.len() != 32 {
        return None;
    }
    let mut k = [0u8; 32];
    for i in 0..32 {
        k[i] = bytes[i] ^ MASK[i];
    }
    Some(k)
}

/// 去遮蔽还原 12 字节 nonce。
pub fn unmask_nonce(b64: &str) -> Option<[u8; 12]> {
    let bytes = crate::b64::decode(b64)?;
    if bytes.len() != 12 {
        return None;
    }
    let mut n = [0u8; 12];
    for i in 0..12 {
        n[i] = bytes[i] ^ MASK[i];
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把十六进制字符串解析为字节向量（仅测试用）。
    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// RFC 8439 §2.3.2 官方测试向量：验证单个 block（keystream）函数。
    /// counter=1，key = 00..1f，nonce = 000000090000004a00000000。
    #[test]
    fn rfc8439_block_vector() {
        let key: [u8; 32] = (0u8..32).collect::<Vec<u8>>().try_into().unwrap();
        let nonce: [u8; 12] = hex("000000090000004a00000000").try_into().unwrap();
        let out = block(&key, &nonce, 1);
        // RFC 8439 §2.3.2 "Serialized Block" 期望输出。
        let expected = hex("10 f1 e7 e4 d1 3b 59 15 50 0f dd 1f a3 20 71 c4 \
             c7 d1 f4 c7 33 c0 68 03 04 22 aa 9a c3 d4 6c 4e \
             d2 82 64 46 07 9f aa 09 14 c2 d7 05 d9 8b 02 a2 \
             b5 12 9c d1 de 16 4e b9 cb d0 83 e8 a2 50 3c 4e");
        assert_eq!(out.to_vec(), expected, "ChaCha20 block 与 RFC 向量不符");
    }

    /// RFC 8439 §2.4.2 官方测试向量：验证完整 keystream + 密文正确。
    /// counter=1，明文为 "Ladies and Gentlemen..." 的英文段落。
    #[test]
    fn rfc8439_encryption_vector() {
        let key: [u8; 32] = (0u8..32).collect::<Vec<u8>>().try_into().unwrap();
        let nonce: [u8; 12] = hex("000000000000004a00000000").try_into().unwrap();
        let plaintext = b"Ladies and Gentlemen of the class of '99: \
If I could offer you only one tip for the future, sunscreen would be it.";
        // RFC 8439 §2.4.2 期望密文。
        let expected = hex("6e 2e 35 9a 25 68 f9 80 41 ba 07 28 dd 0d 69 81 \
             e9 7e 7a ec 1d 43 60 c2 0a 27 af cc fd 9f ae 0b \
             f9 1b 65 c5 52 47 33 ab 8f 59 3d ab cd 62 b3 57 \
             16 39 d6 24 e6 51 52 ab 8f 53 0c 35 9f 08 61 d8 \
             07 ca 0d bf 50 0d 6a 61 56 a3 8e 08 8a 22 b6 5e \
             52 bc 51 4d 16 cc f8 06 81 8c e9 1a b7 79 37 36 \
             5a f9 0b bf 74 a3 5b e6 b4 0b 8e ed f2 78 5e 42 \
             87 4d");
        let ct = encrypt(&key, &nonce, plaintext);
        assert_eq!(ct, expected, "ChaCha20 密文与 RFC 8439 §2.4.2 向量不符");
    }

    #[test]
    fn encrypt_decrypt_roundtrips() {
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        for data in [
            &b""[..],
            b"a",
            b"hello world",
            &[0u8; 63],
            &[0xabu8; 64],
            &[0x11u8; 65],
            &[0x22u8; 200],
        ] {
            let ct = encrypt(&key, &nonce, data);
            if !data.is_empty() {
                assert_ne!(ct, data, "密文不应等于明文");
            }
            let pt = decrypt(&key, &nonce, &ct);
            assert_eq!(pt, data, "解密未还原原文");
        }
    }

    #[test]
    fn mask_unmask_roundtrips() {
        let key = random_key();
        let nonce = random_nonce();
        let kb = mask_key_b64(&key);
        let nb = mask_nonce_b64(&nonce);
        assert_eq!(unmask_key(&kb).unwrap(), key);
        assert_eq!(unmask_nonce(&nb).unwrap(), nonce);
    }

    #[test]
    fn random_bytes_are_not_constant() {
        // obfuscation 级随机：至少两次调用不应完全相同。
        let a = random_key();
        let b = random_key();
        assert_ne!(a, b, "随机 key 两次相同（熵源可能失效）");
    }

    #[test]
    fn unmask_rejects_bad_length() {
        // 12 字节的 nonce base64 不能被当作 key 还原。
        let nonce = random_nonce();
        let nb = mask_nonce_b64(&nonce);
        assert!(unmask_key(&nb).is_none());
    }
}
