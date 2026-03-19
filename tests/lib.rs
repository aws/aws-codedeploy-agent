// Integration tests test your crate's public API. They only have access to items
// in your crate that are marked pub. See the Cargo Targets page of the Cargo Book
// for more information.
//
//   https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests
//
mod appspec;
mod common;
mod security;

#[test]
fn integration_test_appspec_parse() {
    use aws_codedeploy_agent::application_specification::AppSpec;

    let yaml = "version: 0.0\nos: linux\n";
    let spec = AppSpec::parse(yaml).unwrap();
    assert_eq!(spec.version().as_f64(), 0.0);
    assert_eq!(spec.os().as_str(), "linux");
}
