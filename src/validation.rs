use crate::contracts::{Identifier, Revision, SearchTag, Sha256, Text, Timestamp};
use serde::{Deserialize, Deserializer, de::Error};
macro_rules! bounded_string {
    ($ty:ident,$max:expr) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                if s.trim().is_empty() || s.chars().count() > $max {
                    return Err(D::Error::custom(concat!(stringify!($ty), " 为空白或超长")));
                }
                Ok(Self(s))
            }
        }
    };
}
bounded_string!(Identifier, 128);
bounded_string!(Text, 16384);
bounded_string!(SearchTag, 64);
impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if !s.ends_with('Z') || chrono::DateTime::parse_from_rfc3339(&s).is_err() {
            return Err(D::Error::custom("时间必须为 RFC3339 UTC Z 格式"));
        }
        Ok(Self(s))
    }
}
impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() != 64
            || !s
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(D::Error::custom("SHA256 必须 64 位小写十六进制"));
        }
        Ok(Self(s))
    }
}
impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = i64::deserialize(d)?;
        if !(1..=9007199254740991).contains(&n) {
            return Err(D::Error::custom("revision/generation/epoch 越界"));
        }
        Ok(Self(n))
    }
}
