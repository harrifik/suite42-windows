//! PDF Standard Security Handler, revision 3 (RC4, 128-bit): password-protected copies.

use lopdf::{Dictionary, Document, Object};
use md5::{Digest, Md5};

const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08, 0x2E, 0x2E, 0x00,
    0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|b| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            let k = s[(s[i as usize].wrapping_add(s[j as usize])) as usize];
            b ^ k
        })
        .collect()
}

fn pad(pw: &str) -> Vec<u8> {
    // Passwords are Latin-1 in R3; other characters are folded to '?'.
    let mut v: Vec<u8> = pw.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).take(32).collect();
    v.extend_from_slice(&PAD[..32 - v.len()]);
    v
}

fn md5(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = Md5::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

pub struct Permissions {
    pub print: bool,
    pub copy: bool,
    pub modify: bool,
}

/// Encrypts the whole document in place.
pub fn encrypt(doc: &mut Document, user: &str, owner: &str, perms: &Permissions) -> Result<(), String> {
    let owner = if owner.is_empty() { user } else { owner };
    // Owner key
    let mut k = md5(&[&pad(owner)]);
    for _ in 0..50 {
        k = md5(&[&k]);
    }
    let mut o = rc4(&k, &pad(user));
    for i in 1..=19u8 {
        let key: Vec<u8> = k.iter().map(|b| b ^ i).collect();
        o = rc4(&key, &o);
    }
    // Permissions: bits 3 print, 4 modify, 5 copy, 6 annotations, 9 fill forms, 10 extract, 11 assemble, 12 hq print.
    let mut p: u32 = 0xFFFF_F0C0;
    if perms.print {
        p |= 1 << 2 | 1 << 11;
    }
    if perms.modify {
        p |= 1 << 3 | 1 << 5 | 1 << 10;
    }
    if perms.copy {
        p |= 1 << 4 | 1 << 9;
    }
    p |= 1 << 8; // fill forms always allowed
    let id0: Vec<u8> = (0..16).map(|_| rand::random::<u8>()).collect();
    let mut fk = md5(&[&pad(user), &o, &p.to_le_bytes(), &id0]);
    for _ in 0..50 {
        fk = md5(&[&fk]);
    }
    let mut u = rc4(&fk, &md5(&[&PAD, &id0]));
    for i in 1..=19u8 {
        let key: Vec<u8> = fk.iter().map(|b| b ^ i).collect();
        u = rc4(&key, &u);
    }
    u.extend_from_slice(&[0u8; 16]);

    // Encrypt every string and stream with its object key.
    let ids: Vec<_> = doc.objects.keys().cloned().collect();
    for id in ids {
        let mut key = fk.to_vec();
        key.extend_from_slice(&id.0.to_le_bytes()[..3]);
        key.extend_from_slice(&id.1.to_le_bytes()[..2]);
        let ok = md5(&[&key]);
        let ok = &ok[..16.min(fk.len() + 5)];
        if let Some(obj) = doc.objects.get_mut(&id) {
            encrypt_object(obj, ok);
        }
    }
    let mut enc = Dictionary::new();
    enc.set("Filter", Object::Name(b"Standard".to_vec()));
    enc.set("V", 2);
    enc.set("R", 3);
    enc.set("Length", 128);
    enc.set("O", Object::String(o, lopdf::StringFormat::Hexadecimal));
    enc.set("U", Object::String(u, lopdf::StringFormat::Hexadecimal));
    enc.set("P", p as i32 as i64);
    let enc_id = doc.add_object(enc);
    doc.trailer.set("Encrypt", Object::Reference(enc_id));
    let idobj = Object::String(id0, lopdf::StringFormat::Hexadecimal);
    doc.trailer.set("ID", Object::Array(vec![idobj.clone(), idobj]));
    Ok(())
}

fn encrypt_object(obj: &mut Object, key: &[u8]) {
    match obj {
        Object::String(s, _) => *s = rc4(key, s),
        Object::Array(a) => a.iter_mut().for_each(|o| encrypt_object(o, key)),
        Object::Dictionary(d) => d.iter_mut().for_each(|(_, o)| encrypt_object(o, key)),
        Object::Stream(st) => {
            st.dict.iter_mut().for_each(|(_, o)| encrypt_object(o, key));
            let c = rc4(key, &st.content);
            st.set_content(c);
        }
        _ => {}
    }
}
