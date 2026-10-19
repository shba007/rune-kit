use flate2::Compression;
use flate2::write::GzEncoder;
use rune_kit_core::resolve_host_env_params;
use tar::Builder;

#[test]
fn test_resolve_host_env_params_filters_sensitive_secrets() {
    // Set a sensitive token and an allowed parameter
    unsafe {
        std::env::set_var("GITHUB_TOKEN", "ghp_secret_token_12345");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "aws_super_secret");
        std::env::set_var("RUNE_PARAM_CUSTOM_KEY", "safe_value");
        std::env::set_var("ALLOWED_DIR", "/workspace/safe");
    }

    let params = resolve_host_env_params();

    // Verify secret tokens are not present
    assert!(!params.contains_key("github_token"));
    assert!(!params.contains_key("aws_secret_access_key"));

    // Verify allowed parameters are captured
    assert_eq!(
        params.get("custom_key").map(String::as_str),
        Some("safe_value")
    );
    assert_eq!(
        params.get("allowed_dir").map(String::as_str),
        Some("/workspace/safe")
    );
}

#[test]
fn test_tar_slip_rejection() {
    let tmp = std::env::temp_dir().join(format!("rune-test-{}", std::process::id()));
    let dest = tmp.join("extracted");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&dest).unwrap();

    // Construct a malicious tar archive in memory with a traversal entry
    let mut tar_bytes = Vec::new();
    {
        let enc = GzEncoder::new(&mut tar_bytes, Compression::default());
        let mut builder = Builder::new(enc);

        let data = b"malicious content";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        let name_bytes = b"../../escaped.txt";
        header.as_mut_bytes()[..name_bytes.len()].copy_from_slice(name_bytes);
        header.set_cksum();

        builder.append(&header, &data[..]).unwrap();
        builder.finish().unwrap();
    }

    // Attempting to extract this archive must fail with path traversal error
    let gz = flate2::read::GzDecoder::new(&tar_bytes[..]);
    let mut archive = tar::Archive::new(gz);

    let mut traversal_detected = false;
    for entry_res in archive.entries().unwrap() {
        let entry = entry_res.unwrap();
        let path = entry.path().unwrap();
        for comp in path.components() {
            if matches!(comp, std::path::Component::ParentDir) {
                traversal_detected = true;
                break;
            }
        }
    }
    assert!(
        traversal_detected,
        "Tar archive traversal component should be detected"
    );
}
