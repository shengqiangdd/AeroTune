use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn fail(message: impl AsRef<str>) -> ! {
    panic!(
        "AeroTune build security preflight failed: {}",
        message.as_ref()
    );
}

fn ensure_regular_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "expected a regular non-symlink file: {}",
            path.display()
        ));
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, String> {
    ensure_regular_file(path)?;
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&raw).map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

fn validate_windows_installer_overlay(config: &Value, source: &str) -> Result<(), String> {
    let Some(root) = config.as_object() else {
        return Err(format!("{source} must be a JSON object."));
    };
    if let Some(bundle) = root.get("bundle") {
        let Some(bundle) = bundle.as_object() else {
            return Err(format!(
                "{source} must not replace bundle with a non-object."
            ));
        };
        if let Some(targets) = bundle.get("targets") {
            let is_nsis_only = matches!(targets, Value::Array(values)
                if values.len() == 1 && values[0].as_str() == Some("nsis"));
            if !is_nsis_only {
                return Err(format!(
                    "{source} bundle.targets must remain exactly [\"nsis\"] for the AeroTune updater release installer."
                ));
            }
        }
        if let Some(windows) = bundle.get("windows") {
            let Some(windows) = windows.as_object() else {
                return Err(format!("{source} must not replace bundle.windows."));
            };
            if windows.contains_key("sign-command") {
                return Err(format!(
                    "{source} must use the canonical bundle.windows.signCommand key; the serde alias is forbidden."
                ));
            }
            if windows
                .get("signCommand")
                .is_some_and(|value| !value.is_null())
            {
                return Err(format!(
                    "{source} must not set bundle.windows.signCommand; custom code-sign commands are not audited."
                ));
            }
            if windows.contains_key("allow-downgrades") {
                return Err(format!(
                    "{source} must use the canonical bundle.windows.allowDowngrades key; the serde alias is forbidden."
                ));
            }
            if let Some(value) = windows.get("allowDowngrades") {
                if value != &Value::Bool(false) {
                    return Err(format!(
                        "{source} must not enable or erase bundle.windows.allowDowngrades."
                    ));
                }
            }
            if let Some(nsis_value) = windows.get("nsis") {
                let Some(nsis) = nsis_value.as_object() else {
                    return Err(format!("{source} must not replace bundle.windows.nsis."));
                };
                if nsis.contains_key("install-mode")
                    || nsis.contains_key("installer-hooks")
                    || nsis.contains_key("install-dir")
                {
                    return Err(format!(
                        "{source} must use canonical NSIS keys; serde aliases are forbidden."
                    ));
                }
                if nsis.get("installDir").is_some_and(|value| !value.is_null()) {
                    return Err(format!(
                        "{source} must not set bundle.windows.nsis.installDir; preserve the fixed Program Files\\AeroTune path."
                    ));
                }
                if nsis.get("installMode").and_then(Value::as_str) != Some("perMachine") {
                    return Err(format!(
                        "{source} must keep bundle.windows.nsis.installMode perMachine."
                    ));
                }
                if nsis.get("template").is_some_and(|value| !value.is_null()) {
                    return Err(format!(
                        "{source} must not provide a custom NSIS template that can weaken the Program Files install boundary."
                    ));
                }
                if nsis
                    .get("installerHooks")
                    .is_some_and(|value| !value.is_null())
                {
                    return Err(format!(
                        "{source} must not provide custom NSIS installer hooks with elevated script execution."
                    ));
                }
                if nsis
                    .get("customLanguageFiles")
                    .is_some_and(|value| !value.is_null())
                {
                    return Err(format!(
                        "{source} must not provide custom NSIS language source files."
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_release_overlay_scope(config: &Value, source: &str) -> Result<(), String> {
    let Some(root) = config.as_object() else {
        return Err(format!("{source} must be a JSON object."));
    };
    for key in root.keys() {
        if !matches!(key.as_str(), "$schema" | "bundle" | "plugins") {
            return Err(format!(
                "{source} may only override $schema, bundle, and plugins; root key {key} could alter the protected app or build pipeline."
            ));
        }
    }
    if let Some(bundle) = root.get("bundle") {
        let Some(bundle) = bundle.as_object() else {
            return Err(format!("{source} bundle must be an object."));
        };
        for key in bundle.keys() {
            if !matches!(
                key.as_str(),
                "createUpdaterArtifacts" | "targets" | "windows"
            ) {
                return Err(format!(
                    "{source} bundle key {key} is outside the reviewed updater release overlay scope."
                ));
            }
        }
        if let Some(windows) = bundle.get("windows") {
            let Some(windows) = windows.as_object() else {
                return Err(format!("{source} bundle.windows must be an object."));
            };
            for key in windows.keys() {
                if !matches!(
                    key.as_str(),
                    "allowDowngrades" | "allow-downgrades" | "nsis"
                ) {
                    return Err(format!(
                        "{source} bundle.windows key {key} is outside the reviewed updater release overlay scope."
                    ));
                }
            }
            if let Some(nsis) = windows.get("nsis") {
                let Some(nsis) = nsis.as_object() else {
                    return Err(format!("{source} bundle.windows.nsis must be an object."));
                };
                for key in nsis.keys() {
                    if !matches!(
                        key.as_str(),
                        "installMode"
                            | "install-mode"
                            | "installer-hooks"
                            | "template"
                            | "installerHooks"
                            | "customLanguageFiles"
                    ) {
                        return Err(format!(
                            "{source} bundle.windows.nsis key {key} is outside the reviewed NSIS overlay scope."
                        ));
                    }
                }
            }
        }
    }
    if let Some(plugins) = root.get("plugins") {
        let Some(plugins) = plugins.as_object() else {
            return Err(format!("{source} plugins must be an object."));
        };
        for key in plugins.keys() {
            if key != "updater" {
                return Err(format!(
                    "{source} plugin {key} is outside the reviewed updater release overlay scope."
                ));
            }
        }
        if let Some(updater) = plugins.get("updater") {
            let Some(updater) = updater.as_object() else {
                return Err(format!("{source} plugins.updater must be an object."));
            };
            for key in updater.keys() {
                if !matches!(
                    key.as_str(),
                    "pubkey"
                        | "endpoints"
                        | "requireSignedVersion"
                        | "require-signed-version"
                        | "allowDowngrades"
                        | "allow-downgrades"
                        | "dangerousInsecureTransportProtocol"
                        | "dangerous-insecure-transport-protocol"
                        | "dangerousAcceptInvalidCerts"
                        | "dangerous-accept-invalid-certs"
                        | "dangerousAcceptInvalidHostnames"
                        | "dangerous-accept-invalid-hostnames"
                        | "noProxy"
                        | "no_proxy"
                        | "proxy"
                        | "windows"
                ) {
                    return Err(format!(
                        "{source} plugins.updater key {key} is outside the reviewed updater configuration scope."
                    ));
                }
            }
            if let Some(windows) = updater.get("windows") {
                let Some(windows) = windows.as_object() else {
                    return Err(format!(
                        "{source} plugins.updater.windows must be an object."
                    ));
                };
                for key in windows.keys() {
                    if !matches!(
                        key.as_str(),
                        "installMode" | "install-mode" | "installerArgs" | "installer-args"
                    ) {
                        return Err(format!(
                            "{source} plugins.updater.windows key {key} is outside the reviewed updater Windows configuration scope."
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_app_identity(
    config: &Value,
    source: &str,
    require_explicit: bool,
) -> Result<(), String> {
    let Some(root) = config.as_object() else {
        return Err(format!("{source} must be a JSON object."));
    };
    if root.contains_key("product-name") || root.contains_key("main-binary-name") {
        return Err(format!(
            "{source} must use canonical application identity keys; accepted serde aliases are forbidden."
        ));
    }
    if root.contains_key("mainBinaryName") {
        return Err(format!(
            "{source} must not override mainBinaryName; the fixed AeroTune executable is required by the signed NSIS bundle."
        ));
    }
    for (key, expected) in [
        ("productName", "AeroTune"),
        ("identifier", "com.aerotune.desktop"),
        ("version", env!("CARGO_PKG_VERSION")),
    ] {
        let value = root.get(key);
        if value.is_some_and(|value| value.as_str() != Some(expected))
            || (require_explicit && value.is_none())
        {
            return Err(format!(
                "{source} must keep {key} fixed to {expected} for the protected install identity and signed version."
            ));
        }
    }
    Ok(())
}

fn validate_build_hooks(
    config: &Value,
    source: &str,
    require_explicit: bool,
) -> Result<(), String> {
    let Some(root) = config.as_object() else {
        return Err(format!("{source} must be a JSON object."));
    };
    let build = root.get("build");
    let build_object = match build {
        Some(value) => Some(value.as_object().ok_or_else(|| {
            format!("{source} build must remain an object to preserve the trusted frontend build.")
        })?),
        None => None,
    };
    if build_object.is_some_and(|build| {
        build.contains_key("before-build-command") || build.contains_key("before-bundle-command")
    }) {
        return Err(format!(
            "{source} must not use before-build-command or before-bundle-command serde aliases."
        ));
    }
    if build_object
        .and_then(|build| build.get("beforeBundleCommand"))
        .is_some_and(|value| !value.is_null())
    {
        return Err(format!(
            "{source} must not define beforeBundleCommand because it would run with updater signing credentials."
        ));
    }
    let command = build_object.and_then(|build| build.get("beforeBuildCommand"));
    const SAFE_FRONTEND_BUILD_COMMAND: &str = "node scripts/build-frontend-sanitized.mjs";
    if command.is_some_and(|value| value.as_str() != Some(SAFE_FRONTEND_BUILD_COMMAND))
        || (require_explicit && command.is_none())
    {
        return Err(format!(
            "{source} beforeBuildCommand must be {SAFE_FRONTEND_BUILD_COMMAND}, which strips updater signing credentials before npm build scripts run."
        ));
    }
    Ok(())
}

fn path_has_property(value: &Value, segments: &[&str]) -> bool {
    let mut current = value;
    for segment in segments {
        let Some(object) = current.as_object() else {
            return false;
        };
        let Some(next) = object.get(*segment) else {
            return false;
        };
        current = next;
    }
    true
}

fn validate_updater_security(
    value: &Value,
    source: &str,
    require_explicit: bool,
) -> Result<(), String> {
    let Some(updater) = value.pointer("/plugins/updater") else {
        if require_explicit {
            return Err(format!(
                "{source} must explicitly configure the updater plugin safely."
            ));
        }
        return Ok(());
    };
    let Some(updater) = updater.as_object() else {
        return Err(format!("{source} must not replace plugins.updater."));
    };
    for alias in [
        "dangerous-insecure-transport-protocol",
        "dangerous-accept-invalid-certs",
        "dangerous-accept-invalid-hostnames",
        "require-signed-version",
        "allow-downgrades",
    ] {
        if updater.contains_key(alias) {
            return Err(format!(
                "{source} must use the canonical plugins.updater key; the serde alias {alias} is forbidden."
            ));
        }
    }
    let allow_downgrades = updater.get("allowDowngrades");
    if (require_explicit && allow_downgrades != Some(&Value::Bool(false)))
        || allow_downgrades.is_some_and(|value| value != &Value::Bool(false))
    {
        return Err(format!(
            "{source} must keep plugins.updater.allowDowngrades false."
        ));
    }
    let require_signed = updater.get("requireSignedVersion");
    if (require_explicit && require_signed != Some(&Value::Bool(true)))
        || require_signed.is_some_and(|value| value != &Value::Bool(true))
    {
        return Err(format!(
            "{source} must keep plugins.updater.requireSignedVersion true."
        ));
    }
    for option in [
        "dangerousInsecureTransportProtocol",
        "dangerousAcceptInvalidCerts",
        "dangerousAcceptInvalidHostnames",
    ] {
        let setting = updater.get(option);
        if setting.is_some_and(|value| value != &Value::Bool(false))
            || (require_explicit && setting.is_none())
        {
            return Err(format!(
                "{source} must keep plugins.updater.{option} false."
            ));
        }
    }
    if ["noProxy", "no_proxy"].iter().any(|option| {
        updater
            .get(*option)
            .is_some_and(|value| value != &Value::Bool(false))
    }) {
        return Err(format!(
            "{source} must not override the selected Windows system-proxy policy with noProxy."
        ));
    }
    if updater.get("proxy").is_some_and(|value| !value.is_null()) {
        return Err(format!(
            "{source} must not set an explicit updater proxy outside the reviewed system-proxy policy."
        ));
    }
    let windows = match updater.get("windows") {
        Some(value) => Some(
            value
                .as_object()
                .ok_or_else(|| format!("{source} must not replace plugins.updater.windows."))?,
        ),
        None if require_explicit => {
            return Err(format!(
                "{source} must explicitly configure updater Windows options."
            ));
        }
        None => None,
    };
    if let Some(windows) = windows {
        if windows.contains_key("installer-args") || windows.contains_key("install-mode") {
            return Err(format!(
                "{source} must use canonical updater Windows option keys; serde aliases are forbidden."
            ));
        }
        let installer_args = windows.get("installerArgs");
        let installer_args_are_empty =
            matches!(installer_args, Some(Value::Array(args)) if args.is_empty());
        if (installer_args.is_some() && !installer_args_are_empty)
            || (require_explicit && !installer_args_are_empty)
        {
            return Err(format!(
                "{source} must keep plugins.updater.windows.installerArgs an empty array."
            ));
        }
        let install_mode = windows.get("installMode").and_then(Value::as_str);
        if (windows.contains_key("installMode") && install_mode != Some("passive"))
            || (require_explicit && install_mode != Some("passive"))
        {
            return Err(format!(
                "{source} must keep plugins.updater.windows.installMode set to passive so the per-machine installer can request UAC."
            ));
        }
    }
    Ok(())
}

fn updater_is_enabled(value: &Value) -> bool {
    let artifact_mode = value.pointer("/bundle/createUpdaterArtifacts");
    let artifacts = artifact_mode == Some(&Value::Bool(true))
        || artifact_mode.and_then(Value::as_str) == Some("v1Compatible");
    let updater = value.pointer("/plugins/updater");
    let has_endpoints = updater
        .and_then(|config| config.get("endpoints"))
        .is_some_and(|value| match value.as_array() {
            Some(endpoints) => !endpoints.is_empty(),
            None => true,
        });
    let has_public_key = updater
        .and_then(|config| config.get("pubkey"))
        .is_some_and(|value| match value.as_str() {
            Some(key) => !key.trim().is_empty(),
            None => true,
        });
    artifacts || has_endpoints || has_public_key
}

fn contains_updater_permission(value: &Value) -> bool {
    match value {
        Value::String(text) => text.to_ascii_lowercase().contains("updater:"),
        Value::Array(values) => values.iter().any(contains_updater_permission),
        Value::Object(object) => object.values().any(contains_updater_permission),
        _ => false,
    }
}

fn scan_capabilities(directory: &Path) -> Result<(), String> {
    println!("cargo:rerun-if-changed={}", directory.display());
    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        format!(
            "cannot inspect capability directory {}: {error}",
            directory.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "capability directory must be a real directory: {}",
            directory.display()
        ));
    }
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "cannot inspect capability tree {}: {error}",
            directory.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect capability entry: {error}"))?;
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "capability tree contains a symbolic link: {}",
                path.display()
            ));
        }
        if file_type.is_dir() {
            scan_capabilities(&path)?;
        } else if file_type.is_file() {
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase);
            match extension.as_deref() {
                Some("json") => {
                    let content = fs::read_to_string(&path).map_err(|error| {
                        format!("cannot read capability {}: {error}", path.display())
                    })?;
                    let value: Value = serde_json::from_str(&content).map_err(|error| {
                        format!("cannot parse capability {}: {error}", path.display())
                    })?;
                    if contains_updater_permission(&value) {
                        return Err(format!(
                            "updater plugin permissions are forbidden in {}",
                            path.display()
                        ));
                    }
                }
                Some("json5" | "toml") => {
                    return Err(format!(
                        "unsupported capability format cannot be audited safely: {}",
                        path.display()
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn validate_build_security(manifest_dir: &Path) -> Result<(), String> {
    let base_path = manifest_dir.join("tauri.conf.json");
    println!("cargo:rerun-if-changed={}", base_path.display());
    let base = read_json(&base_path)?;
    validate_app_identity(&base, "base config", true)?;
    validate_build_hooks(&base, "base config", true)?;
    validate_windows_installer_overlay(&base, "base config")?;
    if !matches!(base.pointer("/bundle/targets").and_then(Value::as_array), Some(values)
        if values.len() == 1 && values[0].as_str() == Some("nsis"))
    {
        return Err("base bundle.targets must explicitly be [\"nsis\"] for the AeroTune updater release installer.".into());
    }
    validate_updater_security(&base, "base config", true)?;
    if base.pointer("/bundle/windows/allowDowngrades") != Some(&Value::Bool(false)) {
        return Err("base bundle.windows.allowDowngrades must be explicitly false.".into());
    }
    if base
        .pointer("/bundle/windows/nsis/installMode")
        .and_then(Value::as_str)
        != Some("perMachine")
    {
        return Err("base bundle.windows.nsis.installMode must be explicitly perMachine.".into());
    }
    if contains_updater_permission(&base) {
        return Err("base config must not grant updater plugin permissions.".into());
    }
    if updater_is_enabled(&base) {
        return Err(
            "base config must keep updater artifacts, feed, and public key disabled.".into(),
        );
    }

    // Tauri loads platform config overlays from the manifest directory in addition to the base.
    // Reject unsafe values in every local tauri config, not only the currently selected target.
    for entry in fs::read_dir(manifest_dir)
        .map_err(|error| format!("cannot inspect Tauri config directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("cannot inspect Tauri config entry: {error}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let lower_name = name.to_ascii_lowercase();
        if lower_name.starts_with("tauri")
            && (lower_name.ends_with(".json5") || lower_name.ends_with(".toml"))
        {
            return Err(format!(
                "unsupported non-JSON Tauri config cannot be audited: {}",
                path.display()
            ));
        }
        let is_config_candidate = lower_name == "tauri.conf.json"
            || (lower_name.starts_with("tauri.") && lower_name.contains(".conf."));
        if !is_config_candidate || path == base_path {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "Tauri config must not be a symbolic link: {}",
                path.display()
            ));
        }
        if !file_type.is_file() {
            return Err(format!(
                "Tauri config is not a regular file: {}",
                path.display()
            ));
        }
        if !lower_name.ends_with(".conf.json") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let overlay = read_json(&path)?;
        validate_app_identity(&overlay, &path.display().to_string(), false)?;
        validate_build_hooks(&overlay, &path.display().to_string(), false)?;
        validate_updater_security(&overlay, &path.display().to_string(), false)?;
        validate_windows_installer_overlay(&overlay, &path.display().to_string())?;
        if path_has_property(&overlay, &["app", "security", "capabilities"]) {
            return Err(format!(
                "{} must not override app.security.capabilities.",
                path.display()
            ));
        }
        if contains_updater_permission(&overlay) {
            return Err(format!(
                "{} contains updater plugin permissions.",
                path.display()
            ));
        }
        if updater_is_enabled(&overlay) {
            return Err(format!(
                "{} must keep updater artifacts, feed, and public key disabled.",
                path.display()
            ));
        }
    }

    if let Some(raw) = std::env::var_os("TAURI_CONFIG") {
        println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
        let raw = raw
            .to_str()
            .ok_or_else(|| "TAURI_CONFIG is not valid Unicode JSON.".to_string())?;
        let overlay: Value = serde_json::from_str(raw)
            .map_err(|error| format!("TAURI_CONFIG is invalid JSON: {error}"))?;
        validate_app_identity(&overlay, "TAURI_CONFIG", false)?;
        validate_release_overlay_scope(&overlay, "TAURI_CONFIG")?;
        validate_updater_security(&overlay, "TAURI_CONFIG", false)?;
        validate_windows_installer_overlay(&overlay, "TAURI_CONFIG")?;
        if path_has_property(&overlay, &["app", "security", "capabilities"]) {
            return Err("TAURI_CONFIG must not override app.security.capabilities.".into());
        }
        if contains_updater_permission(&overlay) {
            return Err("TAURI_CONFIG must not grant updater plugin permissions.".into());
        }
        if updater_is_enabled(&overlay) {
            return Err(
                "TAURI_CONFIG must keep updater artifacts, feed, and public key disabled.".into(),
            );
        }
    } else {
        println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    }

    for relative in ["src/lib.rs", "src/updater.rs"] {
        let path = manifest_dir.join(relative);
        println!("cargo:rerun-if-changed={}", path.display());
        ensure_regular_file(&path)?;
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if relative == "src/lib.rs" {
            if source
                .matches("tauri_plugin_updater::Builder::new().build()")
                .count()
                != 1
            {
                return Err(
                    "the ordinary app must register exactly one Tauri updater plugin for the Rust IPC façade."
                        .into(),
                );
            }
            if source.contains("update_recovery;") {
                return Err(
                    "the retired custom updater recovery module must not be compiled into the app."
                        .into(),
                );
            }
        }
        if relative == "src/updater.rs" {
            for gate in [
                "const UPDATE_FEED_RESPONSE_BOUNDED_READY: bool =",
                "const UPDATE_PROXY_POLICY_READY: bool =",
                "const UPDATE_BOUNDED_DOWNLOAD_READY: bool =",
                "const GITHUB_RELEASE_HOST_POLICY_REVIEWED: bool =",
                "const GITHUB_RELEASE_WINDOWS_ACCEPTED: bool =",
                "fn github_release_update_readiness_error() -> Option<String>",
                "const PROPOSED_MAX_UPDATE_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;",
                "const PROPOSED_MAX_UPDATE_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;",
                "const MAX_UPDATE_ARCHIVE_ENTRIES: usize = 1;",
                "async fn collect_bounded_update_response(",
                "if !response.status().is_success()",
                "response.content_length()",
                ".chunk()",
                "checked_add(chunk.len())",
                "fn verify_update_signature(",
                "async fn download_and_verify_github_update(",
                "fn build_github_bounded_update_request(",
                "fn github_updater_redirect_is_safe(",
                "fn github_redirect_url_is_allowed(",
                "fn github_release_asset_url_is_allowed(",
                "fn is_github_release_endpoint(",
                "fn validate_github_release_inputs(",
                "fn configured_github_release_inputs(",
                "const GITHUB_RELEASE_ENDPOINT: &str",
                "pub(crate) struct VerifiedUpdateArtifact",
                "fn verify_and_wrap_update_artifact(",
                "fn matches_target_version(",
                "Result<VerifiedUpdateArtifact, String>",
                "|bytes, version| validate_update_archive_payload(bytes, version),",
                "download_and_verify_github_update(&update).await?",
                "artifact.matches_target_version(&update.version)",
                "validate_github_release_asset_url(&update.download_url)?;",
                ".verify(bytes)",
                "fn validate_update_archive_payload(",
                "fn validate_update_archive_with_limits(",
                "fn validate_pe_installer_header(",
                "header.get(..2) != Some(b\"MZ\")",
                "header.get(pe_offset..pe_offset + 4) != Some(&[b'P', b'E', 0, 0][..])",
                "let machine = u16::from_le_bytes([header[pe_offset + 4], header[pe_offset + 5]]);",
                "let expected_machine = if cfg!(target_arch = \"x86_64\")",
                "validate_pe_installer_header(&installer_header)?;",
                "let mut installer_header = Vec::with_capacity(4096);",
                "zip::ZipArchive::new(std::io::Cursor::new(bytes))",
                "entry.encrypted()",
                "entry.is_symlink()",
                "entry.enclosed_name().is_none()",
                "lower_name.starts_with(\"aerotune_\")",
                "lower_name.ends_with(\"-setup.exe\")",
                "actual_total_expanded > expanded_limit",
                ".read(&mut scratch)",
                "fn validate_system_proxy_policy(",
                "validate_system_proxy_policy(update.no_proxy, update.proxy.is_some())?",
                "selected_system_proxy_policy_rejects_direct_and_explicit_proxy_overrides",
                "if !UPDATE_FEED_RESPONSE_BOUNDED_READY",
                "if !GITHUB_RELEASE_HOST_POLICY_REVIEWED",
                "if !UPDATE_PROXY_POLICY_READY",
                "if !UPDATE_BOUNDED_DOWNLOAD_READY",
                "if !GITHUB_RELEASE_WINDOWS_ACCEPTED",
                "fn valid_tauri_public_key(",
                "if !valid_tauri_public_key(public_key)",
                "tauri_public_key_must_decode_to_a_valid_minisign_key",
            ] {
                if !source.contains(gate) {
                    return Err(format!("GitHub updater contract is missing: {gate}"));
                }
            }
            if source.contains("run_update_only")
                || source.contains("UPDATE_ONLY_ARGUMENT")
                || source.contains("tauri_plugin_updater::Builder::new()")
                || source.contains("crate::update_recovery")
                || source.contains("download_and_verify_bounded_update")
                || source.contains("AeroTune-SignedUpdater")
            {
                return Err("retired scheduled-task/recovery updater code must not remain in the standard updater module.".into());
            }
            if source.contains("pub bytes: Vec<u8>")
                || source.contains("pub target_version: semver::Version")
            {
                return Err("verified update artifact internals must remain private.".into());
            }
            let request_builder = source
                .split_once("fn build_github_bounded_update_request(")
                .and_then(|(_, remainder)| remainder.split_once("\n}\n").map(|(body, _)| body))
                .ok_or_else(|| {
                    "GitHub artifact request builder could not be verified".to_string()
                })?;
            if request_builder.contains(".no_proxy()") || request_builder.contains(".proxy(") {
                return Err(
                    "bounded request builder must preserve automatic Windows system-proxy routing."
                        .into(),
                );
            }
            let compact_source: String = source
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            if compact_source.contains("download_and_install(")
                || compact_source.contains(".download(")
                || compact_source.contains("::download(")
                || compact_source.matches(".install(").count() != 1
                || !compact_source.contains("update.install(artifact.bytes())")
            {
                return Err("updates must use the bounded GitHub download and signature/archive verification before the single standard Tauri install(bytes) call.".into());
            }
        }
    }

    let local_updater_manifest = manifest_dir.join("../vendor/tauri-plugin-updater/Cargo.toml");
    let local_updater_source = manifest_dir.join("../vendor/tauri-plugin-updater/src/updater.rs");
    let app_manifest = manifest_dir.join("Cargo.toml");
    for path in [
        &local_updater_manifest,
        &local_updater_source,
        &app_manifest,
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
        ensure_regular_file(path)?;
    }
    let app_manifest_source = fs::read_to_string(&app_manifest)
        .map_err(|error| format!("cannot read {}: {error}", app_manifest.display()))?;
    if !app_manifest_source.contains("tauri-plugin-updater = \"=2.14.0\"")
        || !app_manifest_source
            .contains("tauri-plugin-updater = { path = \"../vendor/tauri-plugin-updater\" }")
    {
        return Err(
            "the updater dependency must remain pinned to AeroTune's reviewed local patch.".into(),
        );
    }
    let local_updater_manifest_source = fs::read_to_string(&local_updater_manifest)
        .map_err(|error| format!("cannot read {}: {error}", local_updater_manifest.display()))?;
    if !local_updater_manifest_source.contains("name = \"tauri-plugin-updater\"")
        || !local_updater_manifest_source.contains("version = \"2.14.0\"")
        || !local_updater_manifest_source
            .contains("default = [\n    \"rustls-tls\",\n    \"system-proxy\",\n    \"zip\",\n]")
        || !local_updater_manifest_source.contains("system-proxy = [\"reqwest/system-proxy\"]")
    {
        return Err(
            "the vendored updater must match the reviewed 2.14.0 pin and preserve its system-proxy feature.".into(),
        );
    }
    let local_updater_source_text = fs::read_to_string(&local_updater_source)
        .map_err(|error| format!("cannot read {}: {error}", local_updater_source.display()))?;
    for gate in [
        "const MAX_UPDATE_MANIFEST_BYTES: usize = 256 * 1024;",
        "struct BoundedJsonAccumulator",
        "response.content_length()",
        "response.chunk().await?",
        "manifest.finish()",
        "read_bounded_update_manifest(res).await?",
        "bounded_manifest_rejects_oversized_content_length_before_reading",
        "bounded_manifest_rejects_unknown_length_stream_over_limit",
        "bounded_manifest_rejects_truncated_content_length",
    ] {
        if !local_updater_source_text.contains(gate) {
            return Err(format!("bounded updater manifest patch is missing: {gate}"));
        }
    }
    if local_updater_source_text.contains("res.json().await") {
        return Err("unbounded updater manifest JSON parsing must not return.".into());
    }

    scan_capabilities(&manifest_dir.join("capabilities"))?;
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-env-changed=AEROTUNE_UPDATE_ENDPOINT");
    println!("cargo:rerun-if-env-changed=AEROTUNE_UPDATE_PUBLIC_KEY");
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is required"),
    );
    if let Err(error) = validate_build_security(&manifest_dir) {
        fail(error);
    }
    tauri_build::build()
}
