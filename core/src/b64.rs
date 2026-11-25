//! Base64 estándar (RFC 4648, con relleno), sin dependencias. Sirve para guardar el
//! estado binario de los plugins dentro del JSON del proyecto.

const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((chunk[0] as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(ALPHA[((n >> 18) & 63) as usize] as char);
        out.push(ALPHA[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHA[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHA[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// None si el texto no es base64 válido.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let mut vals: Vec<u8> = Vec::with_capacity(s.len());
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        vals.push(v);
    }
    if vals.len() % 4 == 1 { return None; }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for ch in vals.chunks(4) {
        let mut n = 0u32;
        for (i, v) in ch.iter().enumerate() {
            n |= (*v as u32) << (18 - 6 * i as u32);
        }
        out.push((n >> 16) as u8);
        if ch.len() > 2 { out.push((n >> 8) as u8); }
        if ch.len() > 3 { out.push(n as u8); }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectores_conocidos() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"M"), "TQ==");
        assert_eq!(encode(b"Ma"), "TWE=");
        assert_eq!(encode(b"Man"), "TWFu");
        assert_eq!(decode("TQ==").unwrap(), b"M");
        assert_eq!(decode("TWE=").unwrap(), b"Ma");
        assert_eq!(decode("TWFu").unwrap(), b"Man");
    }

    #[test]
    fn ida_y_vuelta_de_cualquier_longitud() {
        for len in 0..64usize {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(decode(&encode(&data)).unwrap(), data, "longitud {}", len);
        }
    }

    #[test]
    fn caracter_invalido_da_none() {
        assert!(decode("TW*u").is_none());
    }
}
