//! CRC-32C (Castagnoli), reflected/table-driven form: poly 0x82F63B78,
//! init 0xFFFFFFFF, final XOR 0xFFFFFFFF. See SPEC.md section 2.

const POLY: u32 = 0x82F6_3B78;

fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            if c & 1 == 1 {
                c = POLY ^ (c >> 1);
            } else {
                c >>= 1;
            }
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

pub fn crc32c(data: &[u8]) -> u32 {
    let table = build_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = table[idx] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::crc32c;

    #[test]
    fn known_vector() {
        // "123456789" -> 0xE3069283 is the well-known CRC-32C test vector.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }
}
