use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Confidence(f64);

impl Confidence {
    #[cfg(test)]
    pub fn as_f64(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Confidence {
    type Error = ConfidenceError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ConfidenceError { value })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfidenceError {
    value: f64,
}

impl fmt::Display for ConfidenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "confidence {} is outside the range [0.0, 1.0]",
            self.value
        )
    }
}

impl std::error::Error for ConfidenceError {}

impl Serialize for Confidence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Confidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_must_be_between_zero_and_one() {
        assert_eq!(Confidence::try_from(0.0).unwrap().as_f64(), 0.0);
        assert_eq!(Confidence::try_from(0.5).unwrap().as_f64(), 0.5);
        assert_eq!(Confidence::try_from(1.0).unwrap().as_f64(), 1.0);
        assert!(Confidence::try_from(-0.1).is_err());
        assert!(Confidence::try_from(1.1).is_err());
        assert!(Confidence::try_from(f64::NAN).is_err());
    }

    #[test]
    fn confidence_serializes_as_number() {
        let value = serde_json::to_value(Confidence::try_from(0.9).unwrap()).unwrap();

        assert_eq!(value, 0.9);
    }

    #[test]
    fn confidence_deserialization_validates_range() {
        let confidence: Confidence = serde_json::from_str("0.9").unwrap();

        assert_eq!(confidence.as_f64(), 0.9);
        assert!(serde_json::from_str::<Confidence>("1.1").is_err());
    }
}
