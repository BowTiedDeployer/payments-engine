//! Fixed-point money type.
//!
//! Amounts are stored as a signed count of ten-thousandths, which matches the
//! four decimal places the input is specified to carry. Integer arithmetic
//! keeps every value exact, and all operations that can overflow are checked.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const DECIMALS: usize = 4;
const SCALE: u64 = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Amount(i64);

impl Amount {
    pub const ZERO: Self = Self(0);

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0).map(Self)
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.0.checked_sub(rhs.0).map(Self)
    }

    pub fn is_positive(self) -> bool {
        self.0 > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseAmountError {
    #[error("not a decimal number")]
    Invalid,
    #[error("more than {DECIMALS} decimal places")]
    TooPrecise,
    #[error("out of range")]
    Overflow,
}

impl FromStr for Amount {
    type Err = ParseAmountError;

    /// Accepts `[-]digits[.digits]` with at most four fractional digits.
    /// Anything with more precision is rejected rather than rounded: silently
    /// changing a monetary amount is worse than refusing it.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (negative, unsigned) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let (int_part, frac_part) = match unsigned.split_once('.') {
            Some((int_part, frac_part)) => (int_part, Some(frac_part)),
            None => (unsigned, None),
        };

        if !is_digits(int_part) {
            return Err(ParseAmountError::Invalid);
        }
        let frac_part = match frac_part {
            None => "",
            Some(f) if !is_digits(f) => return Err(ParseAmountError::Invalid),
            Some(f) if f.len() > DECIMALS => return Err(ParseAmountError::TooPrecise),
            Some(f) => f,
        };

        let padding = std::iter::repeat_n(b'0', DECIMALS - frac_part.len());
        let mut value: i64 = 0;
        for digit in int_part.bytes().chain(frac_part.bytes()).chain(padding) {
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(i64::from(digit - b'0')))
                .ok_or(ParseAmountError::Overflow)?;
        }

        Ok(Self(if negative { -value } else { value }))
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

impl fmt::Display for Amount {
    /// Always prints four decimal places, e.g. `1.5000` or `-0.2500`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The sign is written separately: for values between -1 and 0 the
        // integer part is zero and would otherwise lose it.
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:04}", abs / SCALE, abs % SCALE)
    }
}

impl Serialize for Amount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(AmountVisitor)
    }
}

/// Parses straight from the borrowed field, so reading a row doesn't allocate
/// a `String` just to throw it away.
struct AmountVisitor;

impl Visitor<'_> for AmountVisitor {
    type Value = Amount;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a decimal amount with at most {DECIMALS} decimal places")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Amount, E> {
        v.parse().map_err(E::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amount(s: &str) -> Amount {
        s.parse().unwrap()
    }

    #[test]
    fn parses_up_to_four_decimal_places() {
        assert_eq!(amount("1"), Amount(10_000));
        assert_eq!(amount("1.0"), Amount(10_000));
        assert_eq!(amount("1.5"), Amount(15_000));
        assert_eq!(amount("0.0001"), Amount(1));
        assert_eq!(amount("1.2345"), Amount(12_345));
        assert_eq!(amount("007.10"), Amount(71_000));
        assert_eq!(amount("-0.5"), Amount(-5_000));
    }

    #[test]
    fn parses_the_full_range() {
        assert_eq!(amount("922337203685477.5807"), Amount(i64::MAX));
        assert_eq!(amount("-922337203685477.5807"), Amount(-i64::MAX));
    }

    #[test]
    fn rejects_values_out_of_range() {
        let err = "922337203685477.5808".parse::<Amount>();
        assert_eq!(err, Err(ParseAmountError::Overflow));
    }

    #[test]
    fn rejects_more_than_four_decimal_places() {
        let err = "1.23456".parse::<Amount>();
        assert_eq!(err, Err(ParseAmountError::TooPrecise));
    }

    #[test]
    fn rejects_malformed_input() {
        for input in [
            "", ".", "-", "1.", ".5", "+1", "1e3", "1.2.3", "abc", " 1", "1,5",
        ] {
            assert_eq!(
                input.parse::<Amount>(),
                Err(ParseAmountError::Invalid),
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn displays_four_decimal_places() {
        assert_eq!(Amount(15_000).to_string(), "1.5000");
        assert_eq!(Amount(0).to_string(), "0.0000");
        assert_eq!(Amount(1).to_string(), "0.0001");
        assert_eq!(Amount(-2_500).to_string(), "-0.2500");
        assert_eq!(Amount(-12_345).to_string(), "-1.2345");
    }

    #[test]
    fn displays_extremes_without_panicking() {
        assert_eq!(Amount(i64::MAX).to_string(), "922337203685477.5807");
        assert_eq!(Amount(i64::MIN).to_string(), "-922337203685477.5808");
    }

    #[test]
    fn display_round_trips_through_parse() {
        for raw in [0, 1, -1, 9_999, 10_000, -10_001, 123_456_789, i64::MAX] {
            let a = Amount(raw);
            assert_eq!(a.to_string().parse::<Amount>(), Ok(a));
        }
    }

    #[test]
    fn arithmetic_is_checked() {
        assert_eq!(Amount(1).checked_add(Amount(2)), Some(Amount(3)));
        assert_eq!(Amount(1).checked_sub(Amount(2)), Some(Amount(-1)));
        assert_eq!(Amount(i64::MAX).checked_add(Amount(1)), None);
        assert_eq!(Amount(i64::MIN).checked_sub(Amount(1)), None);
    }
}
