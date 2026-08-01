//! Standard base64 encoder.
//!
//! Hand-rolled rather than pulled from crates.io: it is ~30 lines, sits on the hot path for
//! every frame we transmit, and keeping it in-tree removes a supply-chain dependency from
//! the code that formats bytes destined for the user's terminal.

const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `input` as standard base64 with `=` padding, appending into `out`.
///
/// Appends rather than allocates so frame encoding can reuse one buffer across frames.
pub fn encode_into(input: &[u8], out: &mut Vec<u8>) {
    out.reserve(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for c in &mut chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63]);
        out.push(TABLE[(n >> 12) as usize & 63]);
        out.push(TABLE[(n >> 6) as usize & 63]);
        out.push(TABLE[n as usize & 63]);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(TABLE[(n >> 18) as usize & 63]);
            out.push(TABLE[(n >> 12) as usize & 63]);
            out.push(b'=');
            out.push(b'=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(TABLE[(n >> 18) as usize & 63]);
            out.push(TABLE[(n >> 12) as usize & 63]);
            out.push(TABLE[(n >> 6) as usize & 63]);
            out.push(b'=');
        }
        _ => {}
    }
}

pub fn encode(input: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    encode_into(input, &mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: &[u8]) -> String {
        String::from_utf8(encode(b)).unwrap()
    }

    #[test]
    fn rfc4648_vectors() {
        // The canonical RFC 4648 §10 test vectors -- these pin padding behaviour exactly.
        assert_eq!(s(b""), "");
        assert_eq!(s(b"f"), "Zg==");
        assert_eq!(s(b"fo"), "Zm8=");
        assert_eq!(s(b"foo"), "Zm9v");
        assert_eq!(s(b"foob"), "Zm9vYg==");
        assert_eq!(s(b"fooba"), "Zm9vYmE=");
        assert_eq!(s(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn all_byte_values_roundtrip_length() {
        let data: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        let enc = encode(&data);
        assert_eq!(enc.len(), data.len().div_ceil(3) * 4);
        assert!(enc.iter().all(|c| TABLE.contains(c) || *c == b'='));
    }

    #[test]
    fn high_bytes_encode_correctly() {
        assert_eq!(s(&[0xff, 0xff, 0xff]), "////");
        assert_eq!(s(&[0x00, 0x00, 0x00]), "AAAA");
        assert_eq!(s(&[0xfb, 0xff, 0xbf]), "+/+/");
    }

    #[test]
    fn encode_into_appends() {
        let mut buf = b"PREFIX".to_vec();
        encode_into(b"foo", &mut buf);
        assert_eq!(buf, b"PREFIXZm9v");
    }
}
