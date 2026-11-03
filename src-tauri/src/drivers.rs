//! Superseded third-party driver packages in the Windows driver store.
//!
//! Windows keeps every `oemNN.inf` package it has ever staged. When a newer version of the same
//! driver (same original INF name and provider) is installed, the older packages stay on disk.
//! This module lists those, with their size, and deletes one only through `pnputil /delete-driver`
//! *without* `/force`, so Windows itself refuses a package that a device still uses.
//!
//! Deletion cannot be undone from AeroTune. It is recorded in the change journal as non-revertible
//! and says so; Windows can fetch the driver again from Windows Update or the vendor installer.

use serde::Serialize;
use std::path::{Path, PathBuf};

const DRIVER_STORE: &str = r"DriverStore\FileRepository";
/// `pnputil` itself prints at most a few hundred packages; anything beyond this is not a real store.
const MAX_PACKAGES: usize = 5_000;
/// Directory walk cap per package so a corrupted store cannot stall the command.
const MAX_FILES_PER_PACKAGE: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriverPackage {
    /// `oemNN.inf`, the name `pnputil /delete-driver` expects.
    pub published: String,
    pub original: String,
    pub provider: String,
    pub class_name: String,
    /// Driver date as `(year, month, day)`.
    pub date: (u16, u8, u8),
    /// Numeric driver version parts, e.g. `[1, 4, 5, 7]`.
    pub version: Vec<u32>,
    pub version_text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleDriver {
    pub published: String,
    pub original: String,
    pub provider: String,
    pub class_name: String,
    pub version: String,
    pub date: String,
    /// Version that supersedes it (the newest installed package with the same original name).
    pub superseded_by: String,
    /// `None` when the package folder could not be located or measured; never reported as zero.
    pub size_mb: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleDriverReport {
    pub drivers: Vec<StaleDriver>,
    pub total_packages: usize,
    /// Sum of the sizes that could be measured.
    pub measured_mb: f64,
    /// How many listed packages have an unknown size.
    pub unmeasured: usize,
}

/// A published INF name is `oem` + digits + `.inf`; anything else is never passed to `pnputil`.
pub(crate) fn is_published_inf_name(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("oem") else {
        return false;
    };
    let Some(digits) = rest.strip_suffix(".inf") else {
        return false;
    };
    !digits.is_empty() && digits.len() <= 5 && digits.chars().all(|c| c.is_ascii_digit())
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn element_text(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(xml_unescape(block[start..end].trim()))
}

/// Parses `pnputil /enum-drivers /format xml`. Records that lack a name, original name or a
/// parsable version are skipped rather than guessed, so they can never be proposed for deletion.
pub(crate) fn parse_enum_drivers_xml(xml: &str) -> Vec<DriverPackage> {
    let mut packages = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Driver ") {
        let after = &rest[start..];
        let Some(end) = after.find("</Driver>") else {
            break;
        };
        let block = &after[..end];
        rest = &after[end + "</Driver>".len()..];
        if packages.len() >= MAX_PACKAGES {
            break;
        }
        let Some(name_start) = block.find("DriverName=\"") else {
            continue;
        };
        let name_from = name_start + "DriverName=\"".len();
        let Some(name_len) = block[name_from..].find('"') else {
            continue;
        };
        let published = xml_unescape(&block[name_from..name_from + name_len]);
        if !is_published_inf_name(&published) {
            continue;
        }
        let (Some(original), Some(version_field)) = (
            element_text(block, "OriginalName"),
            element_text(block, "DriverVersion"),
        ) else {
            continue;
        };
        let Some((date, version, version_text)) = parse_driver_version(&version_field) else {
            continue;
        };
        packages.push(DriverPackage {
            published: published.to_ascii_lowercase(),
            original: original.to_ascii_lowercase(),
            provider: element_text(block, "ProviderName").unwrap_or_default(),
            class_name: element_text(block, "ClassName").unwrap_or_default(),
            date,
            version,
            version_text,
        });
    }
    packages
}

/// `"MM/DD/YYYY a.b.c.d"` as printed by `pnputil` (the date is always month-first).
fn parse_driver_version(field: &str) -> Option<((u16, u8, u8), Vec<u32>, String)> {
    let mut parts = field.split_whitespace();
    let date_text = parts.next()?;
    let version_text = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let mut d = date_text.split('/');
    let month: u8 = d.next()?.parse().ok()?;
    let day: u8 = d.next()?.parse().ok()?;
    let year: u16 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1990 {
        return None;
    }
    let version: Vec<u32> = version_text
        .split('.')
        .map(|part| part.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    if version.is_empty() || version.len() > 4 {
        return None;
    }
    Some(((year, month, day), version, version_text.to_string()))
}

/// `a` supersedes `b` only when its driver version is strictly higher and it is not older by date.
/// The same version with a different date (a re-signed or rebuilt package) is *not* treated as a
/// replacement: whether the two are interchangeable cannot be known from the listing.
fn is_newer(a: &DriverPackage, b: &DriverPackage) -> bool {
    a.version > b.version && a.date >= b.date
}

/// Packages that have a strictly newer package with the same original INF name and provider.
/// Returns `(stale package, the newest package that supersedes it)`.
pub(crate) fn find_superseded(packages: &[DriverPackage]) -> Vec<(&DriverPackage, &DriverPackage)> {
    let mut out = Vec::new();
    for old in packages {
        let newest = packages
            .iter()
            .filter(|other| {
                other.published != old.published
                    && other.original == old.original
                    && other.provider.eq_ignore_ascii_case(&old.provider)
                    && is_newer(other, old)
            })
            .max_by(|a, b| (&a.version, a.date).cmp(&(&b.version, b.date)));
        if let Some(newest) = newest {
            out.push((old, newest));
        }
    }
    out.sort_by(|a, b| {
        (&a.0.original, a.0.date, &a.0.version).cmp(&(&b.0.original, b.0.date, &b.0.version))
    });
    out
}

fn format_date(date: (u16, u8, u8)) -> String {
    format!("{:04}-{:02}-{:02}", date.0, date.1, date.2)
}

fn windows_directory() -> Option<PathBuf> {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn folder_size_bytes(path: &Path) -> Option<u64> {
    let mut total = 0u64;
    let mut files = 0usize;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry.ok()?;
        if entry.file_type().is_file() {
            files += 1;
            if files > MAX_FILES_PER_PACKAGE {
                return None;
            }
            total = total.saturating_add(entry.metadata().ok()?.len());
        }
    }
    Some(total)
}

/// Finds the repository folder holding exactly this published package by comparing the original
/// INF's bytes with `Windows\INF\oemNN.inf`, which Windows copies from the repository.
fn repository_folder(windows: &Path, package: &DriverPackage) -> Option<PathBuf> {
    let published = std::fs::read(windows.join("INF").join(&package.published)).ok()?;
    let repository = windows.join("System32").join(DRIVER_STORE);
    let prefix = format!("{}_", package.original);
    for entry in std::fs::read_dir(&repository).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.starts_with(&prefix) || !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let candidate = entry.path().join(&package.original);
        if std::fs::read(&candidate).ok().as_deref() == Some(published.as_slice()) {
            return Some(entry.path());
        }
    }
    None
}

pub(crate) fn build_report(
    packages: &[DriverPackage],
    windows: Option<&Path>,
) -> StaleDriverReport {
    let mut drivers = Vec::new();
    let mut measured = 0u64;
    let mut unmeasured = 0usize;
    for (old, newest) in find_superseded(packages) {
        let size = windows
            .and_then(|w| repository_folder(w, old))
            .and_then(|folder| folder_size_bytes(&folder));
        match size {
            Some(bytes) => measured = measured.saturating_add(bytes),
            None => unmeasured += 1,
        }
        drivers.push(StaleDriver {
            published: old.published.clone(),
            original: old.original.clone(),
            provider: old.provider.clone(),
            class_name: old.class_name.clone(),
            version: old.version_text.clone(),
            date: format_date(old.date),
            superseded_by: format!("{} ({})", newest.version_text, format_date(newest.date)),
            size_mb: size.map(|bytes| (bytes as f64 / 1_048_576.0 * 10.0).round() / 10.0),
        });
    }
    StaleDriverReport {
        drivers,
        total_packages: packages.len(),
        measured_mb: (measured as f64 / 1_048_576.0 * 10.0).round() / 10.0,
        unmeasured,
    }
}

fn enumerate_packages() -> Result<Vec<DriverPackage>, String> {
    let xml = crate::util::run_output_checked_with_timeout(
        "pnputil",
        &["/enum-drivers", "/format", "xml"],
        std::time::Duration::from_secs(60),
    )
    .map_err(|error| {
        format!("无法读取驱动包列表（此系统的 pnputil 可能不支持 /format xml）：{error}")
    })?;
    if !xml.contains("<PnpUtil") {
        return Err("pnputil 没有返回可识别的 XML；为避免误判，未列出任何驱动".into());
    }
    let packages = parse_enum_drivers_xml(&xml);
    if packages.is_empty() {
        return Err("没有解析到任何驱动包；为避免误判，未列出任何驱动".into());
    }
    Ok(packages)
}

/// Lists driver packages that a newer installed package supersedes. Read only.
#[tauri::command(async)]
pub fn list_stale_drivers() -> Result<StaleDriverReport, String> {
    let packages = enumerate_packages()?;
    Ok(build_report(&packages, windows_directory().as_deref()))
}

/// Deletes one superseded package. The request is checked against a fresh listing, never against
/// what the renderer last showed, and `/force` is never used, so a package that a device still
/// uses is refused by Windows.
#[tauri::command(async)]
pub fn delete_stale_driver(published: String) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("删除驱动包需要管理员权限".into());
    }
    if !is_published_inf_name(&published) {
        return Err("驱动包名称不合法".into());
    }
    let published = published.to_ascii_lowercase();
    let packages = enumerate_packages()?;
    let target = find_superseded(&packages)
        .into_iter()
        .find(|(old, _)| old.published == published)
        .ok_or_else(|| {
            "该驱动包现在不再被判定为过期（可能已更新或已被删除）；请刷新列表".to_string()
        })?;
    let label = format!(
        "{} {} ({})",
        target.0.original, target.0.version_text, target.0.published
    );
    crate::util::run_checked_with_timeout(
        "pnputil",
        &["/delete-driver", &published],
        std::time::Duration::from_secs(120),
    )
    .map_err(|error| format!("Windows 拒绝删除该驱动包（可能仍有设备在使用）：{error}"))?;
    crate::journal::record(
        &format!("删除过期驱动包 {label}（不可还原；Windows 更新或厂商安装包可重新获取）"),
        crate::journal::Revert::None,
    )?;
    tracing::info!(package = %published, "过期驱动包已删除");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver(published: &str, original: &str, provider: &str, version: &str) -> String {
        format!(
            "<Driver DriverName=\"{published}\"><OriginalName>{original}</OriginalName>\
             <ProviderName>{provider}</ProviderName><ClassName>Media</ClassName>\
             <DriverVersion>{version}</DriverVersion></Driver>"
        )
    }

    fn doc(drivers: &[String]) -> String {
        format!("<PnpUtil Version=\"1\">{}</PnpUtil>", drivers.concat())
    }

    #[test]
    fn published_names_must_be_oem_inf_only() {
        for good in ["oem0.inf", "oem51.inf", "OEM100.INF", "oem12345.inf"] {
            assert!(is_published_inf_name(good), "{good}");
        }
        for bad in [
            "",
            "oem.inf",
            "oem1.sys",
            "oem1inf",
            "my.inf",
            "oem1.inf /force",
            "..\\oem1.inf",
            "C:\\oem1.inf",
            "oem1.inf\n",
            "oem123456.inf",
            "oem-1.inf",
            "oem1.inf;calc",
        ] {
            assert!(!is_published_inf_name(bad), "accepted {bad:?}");
        }
    }

    #[test]
    fn parses_pnputil_xml_and_skips_malformed_records() {
        let xml = doc(&[
            driver(
                "oem1.inf",
                "A.inf",
                "Vendor &amp; Co",
                "06/02/2024 1.0.0.12",
            ),
            driver("oem2.inf", "b.inf", "V", "not a version"),
            driver("evil.inf", "c.inf", "V", "01/01/2024 1.0.0.1"),
            driver("oem3.inf", "d.inf", "V", "13/40/2024 1.0.0.1"),
            "<Driver DriverName=\"oem4.inf\"><ProviderName>V</ProviderName></Driver>".to_string(),
        ]);
        let packages = parse_enum_drivers_xml(&xml);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].published, "oem1.inf");
        assert_eq!(packages[0].original, "a.inf");
        assert_eq!(packages[0].provider, "Vendor & Co");
        assert_eq!(packages[0].date, (2024, 6, 2));
        assert_eq!(packages[0].version, vec![1, 0, 0, 12]);
    }

    #[test]
    fn only_a_strictly_newer_package_makes_an_older_one_stale() {
        let xml = doc(&[
            driver("oem1.inf", "nv.inf", "NVIDIA", "10/25/2025 1.4.5.6"),
            driver("oem2.inf", "nv.inf", "NVIDIA", "03/15/2026 1.4.5.7"),
            driver("oem3.inf", "nv.inf", "NVIDIA", "06/11/2026 1.4.5.7"),
            driver("oem4.inf", "solo.inf", "Acme", "01/01/2024 1.0.0.0"),
        ]);
        let packages = parse_enum_drivers_xml(&xml);
        let stale: Vec<_> = find_superseded(&packages)
            .iter()
            .map(|(old, newest)| (old.published.clone(), newest.published.clone()))
            .collect();
        // oem2 has the same version as oem3 (only the date differs), so it is not proposed.
        assert_eq!(
            stale,
            vec![("oem1.inf".to_string(), "oem3.inf".to_string())]
        );
    }

    #[test]
    fn the_same_version_with_a_different_date_is_not_a_replacement() {
        let xml = doc(&[
            driver("oem1.inf", "nvhda.inf", "NVIDIA", "03/15/2026 1.4.5.7"),
            driver("oem2.inf", "nvhda.inf", "NVIDIA", "06/11/2026 1.4.5.7"),
        ]);
        assert!(find_superseded(&parse_enum_drivers_xml(&xml)).is_empty());
    }

    #[test]
    fn a_higher_version_with_an_older_date_is_not_a_replacement() {
        // Inconsistent metadata: refuse to guess which one is really newer.
        let xml = doc(&[
            driver("oem1.inf", "q.inf", "V", "01/01/2026 1.0.0.0"),
            driver("oem2.inf", "q.inf", "V", "01/01/2020 2.0.0.0"),
        ]);
        assert!(find_superseded(&parse_enum_drivers_xml(&xml)).is_empty());
    }

    #[test]
    fn the_newest_package_and_unrelated_providers_are_never_stale() {
        // Same original name from a different provider is a different driver, not a replacement.
        let xml = doc(&[
            driver("oem1.inf", "x.inf", "Alpha", "01/01/2020 1.0.0.0"),
            driver("oem2.inf", "x.inf", "Beta", "01/01/2026 9.0.0.0"),
        ]);
        let packages = parse_enum_drivers_xml(&xml);
        assert!(find_superseded(&packages).is_empty());
        // Identical date and version is not "newer": neither package may be proposed.
        let tie = doc(&[
            driver("oem1.inf", "y.inf", "V", "01/01/2025 1.0.0.0"),
            driver("oem2.inf", "y.inf", "V", "01/01/2025 1.0.0.0"),
        ]);
        assert!(find_superseded(&parse_enum_drivers_xml(&tie)).is_empty());
    }

    #[test]
    fn version_comparison_is_numeric_not_textual() {
        let xml = doc(&[
            driver("oem1.inf", "z.inf", "V", "01/01/2025 1.9.0.0"),
            driver("oem2.inf", "z.inf", "V", "01/01/2025 1.10.0.0"),
        ]);
        let packages = parse_enum_drivers_xml(&xml);
        let stale = find_superseded(&packages);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].0.published, "oem1.inf");
    }

    #[test]
    fn unknown_sizes_are_reported_as_unknown_not_zero() {
        let xml = doc(&[
            driver("oem1.inf", "w.inf", "V", "01/01/2024 1.0.0.0"),
            driver("oem2.inf", "w.inf", "V", "01/01/2025 2.0.0.0"),
        ]);
        let packages = parse_enum_drivers_xml(&xml);
        let report = build_report(&packages, None);
        assert_eq!(report.drivers.len(), 1);
        assert_eq!(report.drivers[0].size_mb, None);
        assert_eq!(report.unmeasured, 1);
        assert_eq!(report.measured_mb, 0.0);
        assert_eq!(report.total_packages, 2);
    }

    #[test]
    fn the_delete_path_never_forces_and_only_accepts_published_names() {
        let source = include_str!("drivers.rs");
        let delete = source
            .split("pub fn delete_stale_driver")
            .nth(1)
            .and_then(|rest| rest.split("#[cfg(test)]").next())
            .expect("delete command source");
        assert!(delete.contains("\"/delete-driver\""));
        assert!(!delete.contains("/force"));
        assert!(!delete.contains("/uninstall"));
        assert!(delete.contains("is_published_inf_name(&published)"));
        assert!(delete.contains("find_superseded(&packages)"));
    }

    #[test]
    #[ignore = "live read-only query of the real driver store"]
    fn live_driver_store_lists_without_error() {
        let report = list_stale_drivers().expect("pnputil enumeration");
        assert!(report.total_packages > 0);
        assert!(report
            .drivers
            .iter()
            .all(|d| is_published_inf_name(&d.published)));
    }
}
