fn main() {
    println!("cargo:rerun-if-env-changed=HOF_WIFI_PASSWORD");
    embuild::espidf::sysenv::output();
}
