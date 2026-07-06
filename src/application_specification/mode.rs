//! `AppSpec` file permission mode parsing.
use crate::application_specification::ParseError;
use bitflags::bitflags;
use std::str::FromStr;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Mode: u16 {
        const OWNER_READ    = 0o400;
        const OWNER_WRITE   = 0o200;
        const OWNER_EXECUTE = 0o100;
        const GROUP_READ    = 0o040;
        const GROUP_WRITE   = 0o020;
        const GROUP_EXECUTE = 0o010;
        const WORLD_READ    = 0o004;
        const WORLD_WRITE   = 0o002;
        const WORLD_EXECUTE = 0o001;
        const SETUID        = 0o4000;
        const SETGID        = 0o2000;
        const STICKY        = 0o1000;
    }
}

impl Mode {
    /// Parses a Unix permission mode from an octal string.
    ///
    /// # Examples
    /// ```
    /// # use codedeploy_agent::application_specification::Mode;
    /// let mode = Mode::from_octal("755").unwrap();   // rwxr-xr-x
    /// let mode = Mode::from_octal("0644").unwrap();  // rw-r--r--
    /// ```
    ///
    /// # Errors
    /// Returns an error if the octal string is invalid or contains non-octal digits.
    pub fn from_octal(s: &str) -> Result<Self, ParseError> {
        let mut padded = s.to_string();
        while padded.len() < 3 {
            padded.insert(0, '0');
        }

        // Limit to 4 chars max (0000-7777 octal range, ensuring val <= 0o7777)
        if padded.len() > 4 {
            return Err(ParseError::InvalidModeLength(s.to_string()));
        }

        for ch in padded.chars() {
            if !('0'..='7').contains(&ch) {
                return Err(ParseError::InvalidModeCharacter(s.to_string(), ch));
            }
        }

        let val = u16::from_str_radix(&padded, 8)
            .map_err(|_| ParseError::InvalidModeLength(s.to_string()))?;

        // Use from_bits to validate all bits are recognized
        Mode::from_bits(val).ok_or_else(|| ParseError::InvalidModeLength(s.to_string()))
    }

    #[must_use]
    pub fn owner_readable(&self) -> bool {
        self.contains(Mode::OWNER_READ)
    }
    #[must_use]
    pub fn owner_writable(&self) -> bool {
        self.contains(Mode::OWNER_WRITE)
    }
    #[must_use]
    pub fn owner_executable(&self) -> bool {
        self.contains(Mode::OWNER_EXECUTE)
    }
    #[must_use]
    pub fn group_readable(&self) -> bool {
        self.contains(Mode::GROUP_READ)
    }
    #[must_use]
    pub fn group_writable(&self) -> bool {
        self.contains(Mode::GROUP_WRITE)
    }
    #[must_use]
    pub fn group_executable(&self) -> bool {
        self.contains(Mode::GROUP_EXECUTE)
    }
    #[must_use]
    pub fn world_readable(&self) -> bool {
        self.contains(Mode::WORLD_READ)
    }
    #[must_use]
    pub fn world_writable(&self) -> bool {
        self.contains(Mode::WORLD_WRITE)
    }
    #[must_use]
    pub fn world_executable(&self) -> bool {
        self.contains(Mode::WORLD_EXECUTE)
    }
    #[must_use]
    pub fn setuid(&self) -> bool {
        self.contains(Mode::SETUID)
    }
    #[must_use]
    pub fn setgid(&self) -> bool {
        self.contains(Mode::SETGID)
    }
    #[must_use]
    pub fn sticky(&self) -> bool {
        self.contains(Mode::STICKY)
    }
}

impl FromStr for Mode {
    type Err = ParseError;

    /// Parses a Unix permission mode from an octal string.
    ///
    /// # Examples
    /// ```
    /// # use codedeploy_agent::application_specification::Mode;
    /// # use std::str::FromStr;
    /// let mode: Mode = "755".parse().unwrap();   // rwxr-xr-x
    /// let mode: Mode = "0644".parse().unwrap();  // rw-r--r--
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Mode::from_octal(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_755() {
        let mode = Mode::from_octal("755").unwrap();
        assert!(mode.owner_readable());
        assert!(mode.owner_writable());
        assert!(mode.owner_executable());
        assert!(mode.group_readable());
        assert!(!mode.group_writable());
        assert!(mode.group_executable());
        assert!(mode.world_readable());
        assert!(!mode.world_writable());
        assert!(mode.world_executable());
        assert!(!mode.setuid());
        assert!(!mode.setgid());
        assert!(!mode.sticky());
    }

    #[test]
    fn mode_644() {
        let mode = Mode::from_octal("644").unwrap();
        assert!(mode.owner_readable());
        assert!(mode.owner_writable());
        assert!(!mode.owner_executable());
        assert!(mode.group_readable());
        assert!(!mode.group_writable());
        assert!(!mode.group_executable());
        assert!(mode.world_readable());
        assert!(!mode.world_writable());
        assert!(!mode.world_executable());
    }

    #[test]
    fn mode_special_bits() {
        let mode = Mode::from_octal("4755").unwrap();
        assert!(mode.setuid());

        let mode = Mode::from_octal("2755").unwrap();
        assert!(mode.setgid());

        let mode = Mode::from_octal("1755").unwrap();
        assert!(mode.sticky());
    }

    #[test]
    fn mode_padding() {
        assert_eq!(Mode::from_octal("44").unwrap(), Mode::from_octal("044").unwrap());
        assert_eq!(Mode::from_octal("7").unwrap(), Mode::from_octal("007").unwrap());
    }

    #[test]
    fn mode_invalid() {
        assert!(Mode::from_octal("888").is_err());
        assert!(Mode::from_octal("12345").is_err());
        assert!(Mode::from_octal("abc").is_err());
    }

    #[test]
    fn mode_all_bits() {
        let mode = Mode::from_octal("7777").unwrap();

        assert!(mode.owner_readable());
        assert!(mode.owner_writable());
        assert!(mode.owner_executable());
        assert!(mode.group_readable());
        assert!(mode.group_writable());
        assert!(mode.group_executable());
        assert!(mode.world_readable());
        assert!(mode.world_writable());
        assert!(mode.world_executable());
        assert!(mode.setuid());
        assert!(mode.setgid());
        assert!(mode.sticky());
    }

    #[test]
    fn mode_no_bits() {
        let mode = Mode::from_octal("0").unwrap();

        assert!(!mode.owner_readable());
        assert!(!mode.owner_writable());
        assert!(!mode.owner_executable());
        assert!(!mode.setuid());
        assert!(!mode.setgid());
        assert!(!mode.sticky());
    }

    #[test]
    fn mode_boundary_values() {
        // Max valid value: 7777
        assert!(Mode::from_octal("7777").is_ok());
        // Over max: 10000
        assert!(Mode::from_octal("10000").is_err());
        // Max length: 4 chars
        assert!(Mode::from_octal("77777").is_err());
    }

    #[test]
    fn mode_from_str() {
        use std::str::FromStr;
        let mode = Mode::from_str("755").unwrap();
        assert!(mode.owner_readable());
        assert!(mode.owner_executable());
    }

    #[test]
    fn mode_value_exceeds_7777() {
        // 10000 octal = 4096 decimal, which is > 0o7777 (4095)
        assert!(Mode::from_octal("10000").is_err());
    }
}
