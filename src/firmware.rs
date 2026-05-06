//! Firmware archive parsing and firmware image types.
//!
//! This module provides types and functions for working with Crazyflie firmware,
//! either from release zip archives (containing a manifest.json) or raw binary files.
//!
//! The zip parser supports manifest versions 1 and 2.

use std::collections::HashMap;
use std::io::Read;

use anyhow::{bail, Result};
use serde::{Deserialize, Deserializer};

// -- Manifest deserialization types (internal) --

/// Custom deserializer that accepts either a single string or an array of strings.
/// Manifest v1 uses a plain string for `target`, v2 can use either.
fn string_or_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct StringOrVec;

    impl<'de> Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string or a list of strings")
        }

        fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<A>(self, seq: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            Deserialize::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

#[derive(Debug, Clone, Deserialize)]
struct FileInfo {
    #[allow(dead_code)]
    platform: String,
    #[serde(deserialize_with = "string_or_vec")]
    target: Vec<String>,
    #[serde(rename = "type")]
    file_type: String,
    release: String,
    #[allow(dead_code)]
    repository: String,
    #[serde(default)]
    requires: Option<Vec<String>>,
    #[serde(default)]
    provides: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    version: u32,
    #[serde(default)]
    subversion: u32,
    #[serde(default)]
    fw_platform: String,
    #[serde(default)]
    release: String,
    files: HashMap<String, FileInfo>,
}

// -- Public types --

/// Override for the flash start address of a firmware image.
#[derive(Debug, Clone)]
pub enum FlashStartOverride {
    /// Absolute flash address (e.g. 0x08004000)
    Address(u32),
    /// Flash page number
    Page(u16),
}

/// Which target a firmware image is for.
#[derive(Debug, Clone)]
pub enum FlashTarget {
    /// STM32 main processor firmware
    Stm32 {
        /// Optional override for the flash start address
        start_override: Option<FlashStartOverride>,
    },
    /// nRF51 radio processor firmware (or softdevice)
    Nrf51 {
        /// Optional override for the flash start address
        start_override: Option<FlashStartOverride>,
    },
    /// Expansion deck firmware
    Deck {
        /// Deck name, e.g. "bcAI:esp", "bcLighthouse4"
        name: String,
    },
}

impl FlashTarget {
    /// Returns the target name portion (e.g. "stm32", "nrf51", "bcAI:esp").
    pub fn target_name(&self) -> &str {
        match self {
            FlashTarget::Stm32 { .. } => "stm32",
            FlashTarget::Nrf51 { .. } => "nrf51",
            FlashTarget::Deck { name } => name,
        }
    }
}

/// A single firmware image ready to be flashed.
#[derive(Clone)]
pub struct FirmwareImage {
    /// Raw binary data
    pub data: Vec<u8>,
    /// Which target this image is for
    pub target: FlashTarget,
    /// Original file name
    pub file_name: String,
    /// Firmware type string (e.g. "fw", "bootloader+softdevice")
    pub fw_type: String,
    /// Release version string
    pub version: String,
    /// Softdevice/capability requirements (e.g. \["sd-s130"\])
    pub requires: Vec<String>,
    /// Softdevice/capability provisions (e.g. \["sd-s130"\])
    pub provides: Vec<String>,
}

impl FirmwareImage {
    /// Returns a composite key like "stm32-fw", "nrf51-fw", "bcAI:esp-fw".
    ///
    /// This matches the target-type format used in firmware manifests and
    /// is useful for display and selection in clients.
    pub fn target_key(&self) -> String {
        format!("{}-{}", self.target.target_name(), self.fw_type)
    }
}

impl std::fmt::Debug for FirmwareImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirmwareImage")
            .field("target", &self.target)
            .field("fw_type", &self.fw_type)
            .field("file_name", &self.file_name)
            .field("version", &self.version)
            .field("requires", &self.requires)
            .field("provides", &self.provides)
            .field("data_size", &self.data.len())
            .finish()
    }
}

/// Metadata from the firmware archive manifest.
#[derive(Debug, Clone)]
pub struct FirmwareArchiveInfo {
    /// Platform identifier (e.g. "cf2", "bolt", "cf21bl")
    pub platform: String,
    /// Release version string
    pub release: String,
    /// Manifest version (major)
    pub manifest_version: u32,
    /// Manifest subversion (minor)
    pub manifest_subversion: u32,
}

// -- Target classification --

fn classify_target(target_str: &str, file_type: &str) -> FlashTarget {
    match target_str {
        "stm32" => FlashTarget::Stm32 {
            start_override: None,
        },
        "nrf51" => FlashTarget::Nrf51 {
            start_override: None,
        },
        _ => {
            // Deck targets: the target string may be just the deck name (e.g. "bcAI:esp")
            // or the full key may include the type. Strip the type suffix if target contains it.
            let deck_name = if file_type != "fw" {
                format!("{}:{}", target_str, file_type)
            } else {
                target_str.to_string()
            };
            FlashTarget::Deck { name: deck_name }
        }
    }
}

// -- Public API --

/// Parse a firmware zip archive into a list of firmware images and archive metadata.
///
/// Supports manifest versions 1 and 2. The zip must contain a `manifest.json`
/// at the root level.
///
/// # Arguments
/// * `zip_data` - Raw bytes of the zip file
///
/// # Returns
/// A tuple of (archive metadata, list of firmware images)
pub fn parse_firmware_zip(zip_data: &[u8]) -> Result<(FirmwareArchiveInfo, Vec<FirmwareImage>)> {
    let cursor = std::io::Cursor::new(zip_data);
    let mut archive = zip::ZipArchive::new(cursor)?;

    // Extract and parse manifest
    let manifest = {
        let mut manifest_file = archive.by_name("manifest.json")?;
        let mut manifest_data = Vec::new();
        manifest_file.read_to_end(&mut manifest_data)?;
        let manifest: Manifest = serde_json::from_slice(&manifest_data)?;
        manifest
    };

    if manifest.version > 2 {
        bail!(
            "Unsupported manifest version: {}.{}",
            manifest.version,
            manifest.subversion
        );
    }

    let info = FirmwareArchiveInfo {
        platform: manifest.fw_platform.clone(),
        release: manifest.release.clone(),
        manifest_version: manifest.version,
        manifest_subversion: manifest.subversion,
    };

    let mut images = Vec::new();

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if !file.is_file() || file.name() == "manifest.json" {
            continue;
        }

        let file_name = file.name().to_string();

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        let file_info = manifest
            .files
            .get(&file_name)
            .ok_or_else(|| anyhow::anyhow!("File {} not found in manifest", file_name))?;

        for target_str in &file_info.target {
            let target = classify_target(target_str, &file_info.file_type);

            images.push(FirmwareImage {
                data: buffer.clone(),
                target,
                file_name: file_name.clone(),
                fw_type: file_info.file_type.clone(),
                version: file_info.release.clone(),
                requires: file_info.requires.clone().unwrap_or_default(),
                provides: file_info.provides.clone().unwrap_or_default(),
            });
        }
    }

    Ok((info, images))
}

/// Create a firmware image from a raw binary file for a specific target.
///
/// # Arguments
/// * `data` - Raw binary data
/// * `target` - Which target to flash
/// * `file_name` - Name of the source file (for display purposes)
pub fn firmware_from_binary(data: Vec<u8>, target: FlashTarget, file_name: String) -> FirmwareImage {
    FirmwareImage {
        data,
        target,
        file_name,
        fw_type: "fw".to_string(),
        version: "custom".to_string(),
        requires: Vec::new(),
        provides: Vec::new(),
    }
}

/// Filter a list of firmware images to only include those with matching target keys.
///
/// Target keys have the format "target-type", e.g. "stm32-fw", "nrf51-fw", "bcAI:esp-fw".
/// See [`FirmwareImage::target_key`].
pub fn filter_images(images: Vec<FirmwareImage>, selected_keys: &[String]) -> Vec<FirmwareImage> {
    images
        .into_iter()
        .filter(|img| selected_keys.contains(&img.target_key()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manifest_v2(files: &str) -> String {
        format!(
            r#"{{
                "version": 2,
                "subversion": 1,
                "fw_platform": "cf2",
                "release": "2024.01",
                "files": {{ {} }}
            }}"#,
            files
        )
    }

    fn make_manifest_v1(files: &str) -> String {
        format!(
            r#"{{
                "version": 1,
                "files": {{ {} }}
            }}"#,
            files
        )
    }

    #[test]
    fn test_parse_manifest_v2() {
        let json = make_manifest_v2(
            r#"
            "cf2-2024.01.bin": {
                "platform": "cf2",
                "target": "stm32",
                "type": "fw",
                "release": "2024.01",
                "repository": "crazyflie-firmware"
            }
        "#,
        );
        let manifest: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest.version, 2);
        assert_eq!(manifest.subversion, 1);
        assert_eq!(manifest.fw_platform, "cf2");
        let file = manifest.files.get("cf2-2024.01.bin").unwrap();
        assert_eq!(file.target, vec!["stm32"]);
        assert_eq!(file.file_type, "fw");
    }

    #[test]
    fn test_parse_manifest_v1_missing_fields() {
        let json = make_manifest_v1(
            r#"
            "cf2.bin": {
                "platform": "cf2",
                "target": "stm32",
                "type": "fw",
                "release": "2020.01",
                "repository": "crazyflie-firmware"
            }
        "#,
        );
        let manifest: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.subversion, 0); // default
        assert_eq!(manifest.fw_platform, ""); // default
    }

    #[test]
    fn test_parse_manifest_v2_with_requires_provides() {
        let json = make_manifest_v2(
            r#"
            "nrf51-sd.bin": {
                "platform": "cf2",
                "target": ["nrf51"],
                "type": "bootloader+softdevice",
                "release": "2024.01",
                "repository": "crazyflie2-nrf-bootloader",
                "requires": ["sd-s110"],
                "provides": ["sd-s130"]
            }
        "#,
        );
        let manifest: Manifest = serde_json::from_str(&json).unwrap();
        let file = manifest.files.get("nrf51-sd.bin").unwrap();
        assert_eq!(file.requires, Some(vec!["sd-s110".to_string()]));
        assert_eq!(file.provides, Some(vec!["sd-s130".to_string()]));
    }

    #[test]
    fn test_parse_manifest_target_string_or_vec() {
        // Single string target
        let json = make_manifest_v2(
            r#"
            "a.bin": {
                "platform": "cf2",
                "target": "stm32",
                "type": "fw",
                "release": "2024.01",
                "repository": "test"
            }
        "#,
        );
        let manifest: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(
            manifest.files.get("a.bin").unwrap().target,
            vec!["stm32"]
        );

        // Array target
        let json = make_manifest_v2(
            r#"
            "a.bin": {
                "platform": "cf2",
                "target": ["stm32", "nrf51"],
                "type": "fw",
                "release": "2024.01",
                "repository": "test"
            }
        "#,
        );
        let manifest: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(
            manifest.files.get("a.bin").unwrap().target,
            vec!["stm32", "nrf51"]
        );
    }

    #[test]
    fn test_classify_target() {
        match classify_target("stm32", "fw") {
            FlashTarget::Stm32 { .. } => {}
            other => panic!("Expected Stm32, got {:?}", other),
        }
        match classify_target("nrf51", "fw") {
            FlashTarget::Nrf51 { .. } => {}
            other => panic!("Expected Nrf51, got {:?}", other),
        }
        match classify_target("bcAI:esp", "fw") {
            FlashTarget::Deck { name } => assert_eq!(name, "bcAI:esp"),
            other => panic!("Expected Deck, got {:?}", other),
        }
        match classify_target("bcLighthouse4", "fw") {
            FlashTarget::Deck { name } => assert_eq!(name, "bcLighthouse4"),
            other => panic!("Expected Deck, got {:?}", other),
        }
    }

    #[test]
    fn test_firmware_from_binary() {
        let img = firmware_from_binary(
            vec![0x01, 0x02],
            FlashTarget::Stm32 {
                start_override: Some(FlashStartOverride::Address(0x08004000)),
            },
            "test.bin".to_string(),
        );
        assert_eq!(img.data, vec![0x01, 0x02]);
        assert_eq!(img.version, "custom");
        assert_eq!(img.fw_type, "fw");
        assert!(img.requires.is_empty());
    }

    // Helper to create a minimal zip in memory with manifest.json and one binary
    fn create_test_zip(manifest_json: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let buf = Vec::new();
        let cursor = std::io::Cursor::new(buf);
        let mut zip_writer = zip::ZipWriter::new(cursor);

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip_writer
            .start_file("manifest.json", options)
            .unwrap();
        zip_writer.write_all(manifest_json.as_bytes()).unwrap();

        for (name, data) in files {
            zip_writer.start_file(*name, options).unwrap();
            zip_writer.write_all(data).unwrap();
        }

        let cursor = zip_writer.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn test_parse_firmware_zip_v2() {
        let manifest = make_manifest_v2(
            r#"
            "cf2-2024.01.bin": {
                "platform": "cf2",
                "target": "stm32",
                "type": "fw",
                "release": "2024.01",
                "repository": "crazyflie-firmware"
            },
            "cf2_nrf-2024.01.bin": {
                "platform": "cf2",
                "target": "nrf51",
                "type": "fw",
                "release": "2024.01",
                "repository": "crazyflie2-nrf-firmware"
            }
        "#,
        );

        let zip_data = create_test_zip(
            &manifest,
            &[
                ("cf2-2024.01.bin", &[0xDE, 0xAD]),
                ("cf2_nrf-2024.01.bin", &[0xBE, 0xEF]),
            ],
        );

        let (info, images) = parse_firmware_zip(&zip_data).unwrap();
        assert_eq!(info.platform, "cf2");
        assert_eq!(info.release, "2024.01");
        assert_eq!(images.len(), 2);

        let stm32 = images.iter().find(|i| matches!(i.target, FlashTarget::Stm32 { .. })).unwrap();
        assert_eq!(stm32.data, vec![0xDE, 0xAD]);
        assert_eq!(stm32.fw_type, "fw");

        let nrf51 = images.iter().find(|i| matches!(i.target, FlashTarget::Nrf51 { .. })).unwrap();
        assert_eq!(nrf51.data, vec![0xBE, 0xEF]);
    }

    #[test]
    fn test_parse_firmware_zip_v1() {
        let manifest = make_manifest_v1(
            r#"
            "cf2.bin": {
                "platform": "cf2",
                "target": "stm32",
                "type": "fw",
                "release": "2020.01",
                "repository": "crazyflie-firmware"
            }
        "#,
        );

        let zip_data = create_test_zip(&manifest, &[("cf2.bin", &[0x42])]);

        let (info, images) = parse_firmware_zip(&zip_data).unwrap();
        assert_eq!(info.manifest_version, 1);
        assert_eq!(info.manifest_subversion, 0);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].data, vec![0x42]);
    }

    #[test]
    fn test_parse_firmware_zip_rejects_v3() {
        let manifest = r#"{
            "version": 3,
            "subversion": 0,
            "files": {}
        }"#;

        let zip_data = create_test_zip(manifest, &[]);
        let result = parse_firmware_zip(&zip_data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported manifest version"));
    }
}
