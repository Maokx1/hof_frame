use std::process::Command;

#[test]
fn host_binary_reports_that_esp_idf_is_required() {
    let output = Command::new(env!("CARGO_BIN_EXE_firmware"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "firmware requires an ESP-IDF target"
    );
}
