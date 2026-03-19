//! @risk none
//!
//! Lifecycle event types

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleEventType {
    BeforeBlockTraffic,
    AfterBlockTraffic,
    ApplicationStop,
    BeforeInstall,
    AfterInstall,
    ApplicationStart,
    BeforeAllowTraffic,
    AfterAllowTraffic,
    ValidateService,
}

impl FromStr for LifecycleEventType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BeforeBlockTraffic" => Ok(Self::BeforeBlockTraffic),
            "AfterBlockTraffic" => Ok(Self::AfterBlockTraffic),
            "ApplicationStop" => Ok(Self::ApplicationStop),
            "BeforeInstall" => Ok(Self::BeforeInstall),
            "AfterInstall" => Ok(Self::AfterInstall),
            "ApplicationStart" => Ok(Self::ApplicationStart),
            "BeforeAllowTraffic" => Ok(Self::BeforeAllowTraffic),
            "AfterAllowTraffic" => Ok(Self::AfterAllowTraffic),
            "ValidateService" => Ok(Self::ValidateService),
            _ => Err(format!("Unknown lifecycle event: {s}")),
        }
    }
}

impl fmt::Display for LifecycleEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeBlockTraffic => write!(f, "BeforeBlockTraffic"),
            Self::AfterBlockTraffic => write!(f, "AfterBlockTraffic"),
            Self::ApplicationStop => write!(f, "ApplicationStop"),
            Self::BeforeInstall => write!(f, "BeforeInstall"),
            Self::AfterInstall => write!(f, "AfterInstall"),
            Self::ApplicationStart => write!(f, "ApplicationStart"),
            Self::BeforeAllowTraffic => write!(f, "BeforeAllowTraffic"),
            Self::AfterAllowTraffic => write!(f, "AfterAllowTraffic"),
            Self::ValidateService => write!(f, "ValidateService"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_EVENTS: &[&str] = &[
        "BeforeBlockTraffic",
        "AfterBlockTraffic",
        "ApplicationStop",
        "BeforeInstall",
        "AfterInstall",
        "ApplicationStart",
        "BeforeAllowTraffic",
        "AfterAllowTraffic",
        "ValidateService",
    ];

    #[test]
    fn parse_all_events() {
        for name in ALL_EVENTS {
            assert!(name.parse::<LifecycleEventType>().is_ok(), "failed to parse {name}");
        }
    }

    #[test]
    fn parse_unknown() {
        assert!("Cleanup".parse::<LifecycleEventType>().is_err());
    }

    #[test]
    fn display_roundtrip() {
        for name in ALL_EVENTS {
            let event: LifecycleEventType = name.parse().unwrap();
            assert_eq!(&event.to_string(), name);
        }
    }

    #[test]
    fn parse_case_sensitive() {
        assert!("beforeinstall".parse::<LifecycleEventType>().is_err());
        assert!("BEFOREINSTALL".parse::<LifecycleEventType>().is_err());
    }

    #[test]
    fn events_are_hashable() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(LifecycleEventType::BeforeInstall);
        set.insert(LifecycleEventType::BeforeInstall);
        assert_eq!(set.len(), 1);
    }
}
