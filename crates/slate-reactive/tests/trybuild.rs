#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "trybuild stderr varies by platform"
)]
fn compile_fail_tests() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/trybuild/ui/*.rs");
}
