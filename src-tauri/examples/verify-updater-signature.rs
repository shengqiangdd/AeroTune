use base64::{engine::general_purpose::STANDARD, Engine as _};
use minisign_verify::{PublicKey, Signature};
use std::{env, error::Error, fs, path::PathBuf};

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let artifact_path = PathBuf::from(
        args.next()
            .ok_or("usage: verify-updater-signature <archive> <expected-version>")?,
    );
    let expected_version = args
        .next()
        .ok_or("usage: verify-updater-signature <archive> <expected-version>")?
        .into_string()
        .map_err(|_| "expected-version must be valid Unicode")?;
    if args.next().is_some() {
        return Err("unexpected verifier arguments".into());
    }

    let key_material = env::var("AEROTUNE_UPDATE_PUBLIC_KEY")
        .map_err(|_| "AEROTUNE_UPDATE_PUBLIC_KEY is required for signature verification")?;
    let decoded_key = STANDARD
        .decode(key_material.trim())
        .map_err(|_| "configured updater public key is not valid base64")?;
    let decoded_key = std::str::from_utf8(&decoded_key)
        .map_err(|_| "configured updater public key is not valid UTF-8")?;
    let public_key = PublicKey::decode(decoded_key)
        .map_err(|_| "configured updater Minisign public key is invalid")?;

    let mut signature_path = artifact_path.as_os_str().to_os_string();
    signature_path.push(".sig");
    let encoded_signature = fs::read_to_string(PathBuf::from(signature_path))?;
    let decoded_signature = STANDARD.decode(encoded_signature.trim())?;
    let signature_text = std::str::from_utf8(&decoded_signature)?;
    let signature = Signature::decode(signature_text)?;
    let artifact = fs::read(&artifact_path)?;

    // Verify before trusting the signed comment, then enforce the version that the manifest will announce.
    public_key.verify(&artifact, &signature, true)?;
    let signed_version = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .ok_or("updater signature does not bind a signed app version")?;
    if signed_version != expected_version {
        return Err(format!(
            "updater signature binds version {signed_version}, expected {expected_version}"
        )
        .into());
    }

    println!(
        "PASS updater archive Minisign signature and signed version {expected_version} match the configured public key."
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("FAIL updater signature verification: {error}");
        std::process::exit(1);
    }
}
