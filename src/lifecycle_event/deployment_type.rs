//! Deployment type enums

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentType {
    BlueGreen,
    InPlace,
}

impl FromStr for DeploymentType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BLUE_GREEN" => Ok(Self::BlueGreen),
            "IN_PLACE" => Ok(Self::InPlace),
            _ => Err(format!("Unknown deployment type: {s}")),
        }
    }
}

impl fmt::Display for DeploymentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlueGreen => write!(f, "BLUE_GREEN"),
            Self::InPlace => write!(f, "IN_PLACE"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_blue_green() {
        assert_eq!("BLUE_GREEN".parse::<DeploymentType>().unwrap(), DeploymentType::BlueGreen);
    }

    #[test]
    fn parse_in_place() {
        assert_eq!("IN_PLACE".parse::<DeploymentType>().unwrap(), DeploymentType::InPlace);
    }

    #[test]
    fn parse_unknown() {
        assert!("ROLLING".parse::<DeploymentType>().is_err());
    }

    #[test]
    fn display_roundtrip() {
        assert_eq!(DeploymentType::BlueGreen.to_string(), "BLUE_GREEN");
        assert_eq!(DeploymentType::InPlace.to_string(), "IN_PLACE");
    }

    #[test]
    fn parse_display_roundtrip() {
        for s in &["BLUE_GREEN", "IN_PLACE"] {
            assert_eq!(&s.parse::<DeploymentType>().unwrap().to_string(), s);
        }
    }
}
