//! Decimal strings preserve IEEE-754 inputs through the private JSON ledger
//! without relying on a JSON parser's optional floating-point roundtrip mode.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn parse<E: serde::de::Error>(value: String) -> Result<f64, E> {
    value
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| E::custom("invalid frozen number"))
}
pub(crate) mod scalar {
    use super::*;
    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        if !value.is_finite() {
            return Err(serde::ser::Error::custom("invalid frozen number"));
        }
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        parse(String::deserialize(deserializer)?)
    }
}
pub(crate) mod pair {
    use super::*;
    pub fn serialize<S: Serializer>(value: &[f64; 2], serializer: S) -> Result<S::Ok, S::Error> {
        if value.iter().any(|n| !n.is_finite()) {
            return Err(serde::ser::Error::custom("invalid frozen number"));
        }
        [value[0].to_string(), value[1].to_string()].serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[f64; 2], D::Error> {
        let [a, b] = <[String; 2]>::deserialize(deserializer)?;
        Ok([parse(a)?, parse(b)?])
    }
}
