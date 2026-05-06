//! Nordic softdevice detection and compatibility checking.
//!
//! The nRF51 on Crazyflie 2.x can have a Nordic softdevice installed (S110 or S130).
//! The softdevice occupies the lower flash pages, pushing the firmware start page up.
//! This module detects which softdevice is installed based on the nRF51 bootloader's
//! reported `flash_start` page, and validates firmware compatibility.

use anyhow::{bail, Result};

/// Known Nordic softdevice variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Softdevice {
    /// No softdevice installed (standard start page, typically 16)
    None,
    /// S110 softdevice (start_page == 88)
    S110,
    /// S130 softdevice (start_page == 108)
    S130,
    /// Unknown start page that doesn't match known softdevice layouts
    Unknown(u16),
}

/// The standard nRF51 flash start page when no softdevice is present.
const NRF51_DEFAULT_START_PAGE: u16 = 16;
/// Flash start page when S110 softdevice is installed.
const S110_START_PAGE: u16 = 88;
/// Flash start page when S130 softdevice is installed.
const S130_START_PAGE: u16 = 108;

impl Softdevice {
    /// Detect the installed softdevice from the nRF51 bootloader's flash start page.
    ///
    /// The flash_start page is reported by the nRF51 bootloader's `InfoPacket`.
    /// Different softdevice versions occupy different amounts of flash, resulting
    /// in different start pages.
    pub fn from_start_page(start_page: u16) -> Self {
        match start_page {
            NRF51_DEFAULT_START_PAGE => Softdevice::None,
            S110_START_PAGE => Softdevice::S110,
            S130_START_PAGE => Softdevice::S130,
            other => Softdevice::Unknown(other),
        }
    }

    /// Check if firmware requirements are compatible with the installed softdevice.
    ///
    /// Firmware images may declare `requires` (what softdevice they need) and
    /// `provides` (what softdevice they install). This function checks that
    /// the requirements can be satisfied.
    ///
    /// # Arguments
    /// * `installed` - The currently installed softdevice
    /// * `requires` - What the firmware requires (from `FirmwareImage::requires`)
    /// * `provides` - What the firmware provides (from `FirmwareImage::provides`)
    ///
    /// # Returns
    /// * `Ok(true)` if the softdevice needs to be updated (provides != installed)
    /// * `Ok(false)` if no softdevice update is needed
    /// * `Err` if the requirements are incompatible
    pub fn check_compatibility(
        installed: &Softdevice,
        requires: &[String],
        provides: &[String],
    ) -> Result<bool> {
        // No requirements means no softdevice dependency
        if requires.is_empty() && provides.is_empty() {
            return Ok(false);
        }

        // If firmware provides a softdevice, check if it's different from installed
        if !provides.is_empty() {
            let provides_sd = Self::from_provides_string(provides);
            if let Some(provided) = provides_sd {
                if *installed != provided {
                    // Softdevice update needed
                    return Ok(true);
                }
            }
            return Ok(false);
        }

        // If firmware requires a softdevice, check it matches installed
        if !requires.is_empty() {
            let required_sd = Self::from_requires_string(requires);
            if let Some(required) = required_sd {
                if *installed != required {
                    bail!(
                        "Firmware requires {:?} softdevice but {:?} is installed",
                        required,
                        installed
                    );
                }
            }
        }

        Ok(false)
    }

    /// Parse a softdevice variant from provides strings (e.g. ["sd-s130"]).
    fn from_provides_string(provides: &[String]) -> Option<Softdevice> {
        for p in provides {
            let lower = p.to_lowercase();
            if lower.contains("s110") {
                return Some(Softdevice::S110);
            }
            if lower.contains("s130") {
                return Some(Softdevice::S130);
            }
        }
        None
    }

    /// Parse a softdevice variant from requires strings (e.g. ["sd-s110"]).
    fn from_requires_string(requires: &[String]) -> Option<Softdevice> {
        // Same logic as provides
        Self::from_provides_string(requires)
    }
}

impl std::fmt::Display for Softdevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Softdevice::None => write!(f, "None"),
            Softdevice::S110 => write!(f, "S110"),
            Softdevice::S130 => write!(f, "S130"),
            Softdevice::Unknown(page) => write!(f, "Unknown (start_page={})", page),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_start_page() {
        assert_eq!(Softdevice::from_start_page(16), Softdevice::None);
        assert_eq!(Softdevice::from_start_page(88), Softdevice::S110);
        assert_eq!(Softdevice::from_start_page(108), Softdevice::S130);
        assert_eq!(Softdevice::from_start_page(42), Softdevice::Unknown(42));
    }

    #[test]
    fn test_compatibility_no_requirements() {
        let result = Softdevice::check_compatibility(&Softdevice::S130, &[], &[]);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // no update needed
    }

    #[test]
    fn test_compatibility_requires_matching() {
        let result = Softdevice::check_compatibility(
            &Softdevice::S130,
            &["sd-s130".to_string()],
            &[],
        );
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_compatibility_requires_mismatch() {
        let result = Softdevice::check_compatibility(
            &Softdevice::S110,
            &["sd-s130".to_string()],
            &[],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_compatibility_provides_update_needed() {
        let result = Softdevice::check_compatibility(
            &Softdevice::S110,
            &[],
            &["sd-s130".to_string()],
        );
        assert!(result.is_ok());
        assert!(result.unwrap()); // update needed: S110 -> S130
    }

    #[test]
    fn test_compatibility_provides_already_installed() {
        let result = Softdevice::check_compatibility(
            &Softdevice::S130,
            &[],
            &["sd-s130".to_string()],
        );
        assert!(result.is_ok());
        assert!(!result.unwrap()); // already installed
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Softdevice::None), "None");
        assert_eq!(format!("{}", Softdevice::S110), "S110");
        assert_eq!(format!("{}", Softdevice::S130), "S130");
        assert_eq!(
            format!("{}", Softdevice::Unknown(42)),
            "Unknown (start_page=42)"
        );
    }
}
