//! Minimal hand-built ABI encoding and decoding.
//!
//! Only what the svZCHF reads need: function selectors (keccak256 of the
//! signature, first 4 bytes), a single uint256 argument, and decoding of a
//! single 32 byte return word. No ABI JSON file is involved.

use serde::Serialize;
use tiny_keccak::{Hasher, Keccak};

pub fn keccak256(input: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut hasher = Keccak::v256();
    hasher.update(input);
    hasher.finalize(&mut out);
    out
}

pub fn selector(signature: &str) -> [u8; 4] {
    let hash = keccak256(signature.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    out
}

pub fn hex_decode(input: &str) -> Option<Vec<u8>> {
    let body = input.strip_prefix("0x").unwrap_or(input);
    if !body.len().is_multiple_of(2) {
        return None;
    }
    let bytes = body.as_bytes();
    let mut out = Vec::with_capacity(body.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// Calldata for a zero argument function, as a 0x prefixed hex string.
pub fn encode_no_args(signature: &str) -> String {
    format!("0x{}", hex_encode(&selector(signature)))
}

/// Calldata for a function taking a single uint256, value limited to u128.
/// Every argument passed today is 1e18, well inside u128.
pub fn encode_uint256(signature: &str, value: u128) -> String {
    let mut word = [0u8; 32];
    word[16..32].copy_from_slice(&value.to_be_bytes());
    format!(
        "0x{}{}",
        hex_encode(&selector(signature)),
        hex_encode(&word)
    )
}

/// Calldata for a function taking a single address.
pub fn encode_address(signature: &str, address: &str) -> Result<String, String> {
    let bytes = hex_decode(address).ok_or_else(|| format!("{address} is not hex"))?;
    if bytes.len() != 20 {
        return Err(format!("{address} is not 20 bytes"));
    }
    let mut word = [0u8; 32];
    word[12..32].copy_from_slice(&bytes);
    Ok(format!(
        "0x{}{}",
        hex_encode(&selector(signature)),
        hex_encode(&word)
    ))
}

/// Big endian two's complement 256 bit word to a signed decimal string.
/// A negative value is negated by taking the two's complement, which is exact
/// for every value except the minimum, where the negation is itself.
pub fn word_to_signed_decimal(word: &[u8; 32]) -> String {
    if word[0] & 0x80 == 0 {
        return word_to_decimal(word);
    }
    let mut negated = [0u8; 32];
    for (index, byte) in word.iter().enumerate() {
        negated[index] = !byte;
    }
    // Add one, with carry, to complete the two's complement.
    for byte in negated.iter_mut().rev() {
        let (value, overflowed) = byte.overflowing_add(1);
        *byte = value;
        if !overflowed {
            break;
        }
    }
    format!("-{}", word_to_decimal(&negated))
}

/// Big endian unsigned 256 bit word to a decimal string, by repeated long
/// division over eight 32 bit limbs. Avoids pulling in a bignum dependency.
pub fn word_to_decimal(word: &[u8; 32]) -> String {
    let mut limbs = [0u32; 8];
    for (i, limb) in limbs.iter_mut().enumerate() {
        *limb = u32::from_be_bytes([
            word[i * 4],
            word[i * 4 + 1],
            word[i * 4 + 2],
            word[i * 4 + 3],
        ]);
    }
    let mut digits = Vec::new();
    while limbs.iter().any(|limb| *limb != 0) {
        let mut remainder: u64 = 0;
        for limb in limbs.iter_mut() {
            let current = (remainder << 32) | (*limb as u64);
            *limb = (current / 10) as u32;
            remainder = current % 10;
        }
        digits.push(b'0' + remainder as u8);
    }
    if digits.is_empty() {
        return "0".to_string();
    }
    digits.reverse();
    String::from_utf8(digits).expect("decimal digits are ascii")
}

/// Left pads to an address. Returns None when the top 12 bytes are not zero,
/// which means the word is not a valid ABI encoded address.
pub fn word_to_address(word: &[u8; 32]) -> Option<String> {
    if word[0..12].iter().any(|byte| *byte != 0) {
        return None;
    }
    Some(format!("0x{}", hex_encode(&word[12..32])))
}

/// What the fetch plan expects a read to return. Small integers are
/// indistinguishable from addresses once ABI encoded, so the expected type is
/// declared by the caller rather than guessed from the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Uint,
    Address,
    /// Two's complement signed integer, as int192 and int256 are encoded.
    Int,
}

/// One word of a multi word return value. ABI packs uint192, uint64 and
/// uint32 each into its own 32 byte word, so a field is always one word here.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub kind: FieldKind,
}

#[derive(Debug, Clone, Copy)]
pub enum Expect {
    Uint,
    Address,
    /// A fixed size tuple of single word fields, in ABI order.
    Fields(&'static [Field]),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Decoded {
    /// Zero length return data. On a contract with no matching function this
    /// is what some nodes return instead of a revert.
    Empty,
    Word {
        hex: String,
        decimal: String,
        /// Present only when the fetch plan declared this read to return an
        /// address. It is never inferred from the value.
        #[serde(skip_serializing_if = "Option::is_none")]
        address: Option<String>,
    },
    /// A decoded fixed size tuple of single word fields.
    Fields {
        hex: String,
        fields: Vec<DecodedField>,
    },
    /// Anything that is not exactly one word: recorded, not interpreted.
    Other { hex: String, byte_len: usize },
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodedField {
    pub name: &'static str,
    pub hex: String,
    pub decimal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

pub fn decode_return(data: &str, expect: Expect) -> Decoded {
    let bytes = match hex_decode(data) {
        Some(bytes) => bytes,
        None => {
            return Decoded::Other {
                hex: data.to_string(),
                byte_len: 0,
            }
        }
    };
    if bytes.is_empty() {
        return Decoded::Empty;
    }
    if let Expect::Fields(fields) = expect {
        // A short or long return means the tuple shape assumed here does not
        // match what the contract returned. That is recorded as raw bytes
        // rather than decoded into the wrong shape.
        if bytes.len() == fields.len() * 32 {
            let decoded = fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let mut word = [0u8; 32];
                    word.copy_from_slice(&bytes[index * 32..(index + 1) * 32]);
                    DecodedField {
                        name: field.name,
                        hex: format!("0x{}", hex_encode(&word)),
                        decimal: match field.kind {
                            FieldKind::Int => word_to_signed_decimal(&word),
                            _ => word_to_decimal(&word),
                        },
                        address: match field.kind {
                            FieldKind::Address => word_to_address(&word),
                            FieldKind::Uint | FieldKind::Int => None,
                        },
                    }
                })
                .collect();
            return Decoded::Fields {
                hex: format!("0x{}", hex_encode(&bytes)),
                fields: decoded,
            };
        }
        return Decoded::Other {
            hex: format!("0x{}", hex_encode(&bytes)),
            byte_len: bytes.len(),
        };
    }
    if bytes.len() == 32 {
        let mut word = [0u8; 32];
        word.copy_from_slice(&bytes);
        return Decoded::Word {
            hex: format!("0x{}", hex_encode(&word)),
            decimal: word_to_decimal(&word),
            address: match expect {
                Expect::Address => word_to_address(&word),
                // Fields is handled above; a one word return declared as a
                // tuple never reaches here.
                Expect::Uint | Expect::Fields(_) => None,
            },
        };
    }
    Decoded::Other {
        hex: format!("0x{}", hex_encode(&bytes)),
        byte_len: bytes.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent check of the keccak implementation against selectors that
    /// are publicly documented for ERC-20 and ERC-4626.
    #[test]
    fn known_selectors_match() {
        assert_eq!(encode_no_args("totalSupply()"), "0x18160ddd");
        assert_eq!(encode_no_args("asset()"), "0x38d52e0f");
        assert_eq!(encode_no_args("totalAssets()"), "0x01e1d114");
        assert_eq!(
            hex_encode(&selector("convertToAssets(uint256)")),
            "07a2d13a"
        );
        assert_eq!(encode_no_args("transfer(address,uint256)"), "0xa9059cbb");
    }

    #[test]
    fn keccak_of_empty_input_matches_published_value() {
        assert_eq!(
            hex_encode(&keccak256(b"")),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn encodes_one_ether_argument() {
        assert_eq!(
            encode_uint256("convertToAssets(uint256)", 1_000_000_000_000_000_000u128),
            "0x07a2d13a0000000000000000000000000000000000000000000000000de0b6b3a7640000"
        );
    }

    #[test]
    fn decimal_conversion_covers_full_width() {
        let mut word = [0u8; 32];
        word[24..32].copy_from_slice(&1_000_000_000_000_000_000u64.to_be_bytes());
        assert_eq!(word_to_decimal(&word), "1000000000000000000");

        assert_eq!(word_to_decimal(&[0u8; 32]), "0");

        let max = [0xffu8; 32];
        assert_eq!(
            word_to_decimal(&max),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
    }

    #[test]
    fn decodes_an_address_word() {
        let data = "0x000000000000000000000000b58e61c3098d85632df34eecfb899a1ed80921cb";
        match decode_return(data, Expect::Address) {
            Decoded::Word { address, .. } => assert_eq!(
                address.as_deref(),
                Some("0xb58e61c3098d85632df34eecfb899a1ed80921cb")
            ),
            other => panic!("expected a word, got {other:?}"),
        }
    }

    /// A small integer left pads to something that looks exactly like an
    /// address. Only the declared expectation separates the two, and a read
    /// declared as a uint must never grow an address field.
    #[test]
    fn a_uint_is_never_reported_as_an_address() {
        let one_ether_ish = "0x0000000000000000000000000000000000000000000000000df56462ddd43f40";
        match decode_return(one_ether_ish, Expect::Uint) {
            Decoded::Word {
                address, decimal, ..
            } => {
                assert_eq!(address, None);
                assert_eq!(decimal, "1005820467578421056");
            }
            other => panic!("expected a word, got {other:?}"),
        }
    }

    #[test]
    fn encodes_an_address_argument() {
        assert_eq!(
            encode_address(
                "savings(address)",
                "0xE5F130253fF137f9917C0107659A4c5262abf6b0"
            )
            .unwrap(),
            "0x1f7cdd5f000000000000000000000000e5f130253ff137f9917c0107659a4c5262abf6b0"
        );
        assert!(encode_address("savings(address)", "0x1234").is_err());
    }

    /// The observed return of module.savings(vault) at block 24570000.
    #[test]
    fn decodes_the_savings_account_tuple() {
        const ACCOUNT: [Field; 4] = [
            Field {
                name: "saved",
                kind: FieldKind::Uint,
            },
            Field {
                name: "ticks",
                kind: FieldKind::Uint,
            },
            Field {
                name: "referrer",
                kind: FieldKind::Address,
            },
            Field {
                name: "referralFeePPM",
                kind: FieldKind::Uint,
            },
        ];
        let data = "0x00000000000000000000000000000000000000000000000a20ebd34ab59424bc000000000000000000000000000000000000000000000000000000b8e6679b1900000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        match decode_return(data, Expect::Fields(&ACCOUNT)) {
            Decoded::Fields { fields, .. } => {
                assert_eq!(fields.len(), 4);
                assert_eq!(fields[0].name, "saved");
                assert_eq!(fields[0].decimal, "186839662683663639740");
                assert_eq!(fields[1].decimal, "794139532057");
                assert_eq!(
                    fields[2].address.as_deref(),
                    Some("0x0000000000000000000000000000000000000000")
                );
                assert_eq!(fields[3].decimal, "0");
            }
            other => panic!("expected fields, got {other:?}"),
        }
    }

    /// A tuple that does not have the assumed width must not be decoded into
    /// the wrong shape.
    #[test]
    fn a_tuple_of_the_wrong_width_stays_raw() {
        const ACCOUNT: [Field; 4] = [
            Field {
                name: "saved",
                kind: FieldKind::Uint,
            },
            Field {
                name: "ticks",
                kind: FieldKind::Uint,
            },
            Field {
                name: "referrer",
                kind: FieldKind::Address,
            },
            Field {
                name: "referralFeePPM",
                kind: FieldKind::Uint,
            },
        ];
        let one_word = "0x0000000000000000000000000000000000000000000000000000000000000001";
        assert!(matches!(
            decode_return(one_word, Expect::Fields(&ACCOUNT)),
            Decoded::Other { .. }
        ));
    }

    #[test]
    fn signed_words_decode_as_twos_complement() {
        let mut minus_one = [0xffu8; 32];
        assert_eq!(word_to_signed_decimal(&minus_one), "-1");
        minus_one[31] = 0xfe;
        assert_eq!(word_to_signed_decimal(&minus_one), "-2");

        let mut positive = [0u8; 32];
        positive[31] = 5;
        assert_eq!(word_to_signed_decimal(&positive), "5");
        assert_eq!(word_to_signed_decimal(&[0u8; 32]), "0");

        // A large positive answer must not be read as negative.
        let hex =
            hex_decode("0000000000000000000000000000000000000000000000000000000006615037").unwrap();
        let mut word = [0u8; 32];
        word.copy_from_slice(&hex);
        assert_eq!(word_to_signed_decimal(&word), "107040823");
    }

    #[test]
    fn empty_return_data_is_recognised() {
        assert!(matches!(decode_return("0x", Expect::Uint), Decoded::Empty));
    }
}
