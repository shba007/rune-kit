use rune_kit_core::protocol::{namespace_resource_uri, parse_resource_uri};

#[test]
fn resource_uri_round_trip() {
    let uri = namespace_resource_uri("my-plugin", "config.json");
    assert_eq!(uri, "rune://my-plugin/config.json");
    let (ns, local) = parse_resource_uri(&uri).unwrap();
    assert_eq!(ns, "my-plugin");
    assert_eq!(local, "config.json");
}

#[test]
fn resource_uri_round_trip_with_nested_path() {
    let uri = namespace_resource_uri("my-plugin", "sub/dir/data.json");
    let (ns, local) = parse_resource_uri(&uri).unwrap();
    assert_eq!(ns, "my-plugin");
    assert_eq!(local, "sub/dir/data.json");
}

#[test]
fn parse_resource_uri_rejects_unnamespaced() {
    assert_eq!(parse_resource_uri("config.json"), None);
    assert_eq!(parse_resource_uri("rune://only-namespace"), None);
}
