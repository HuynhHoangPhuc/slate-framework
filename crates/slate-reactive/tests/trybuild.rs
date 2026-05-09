#[test]
#[cfg(target_os = "linux")]
fn compile_fail_tests() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/trybuild/ui/*.rs");
}
