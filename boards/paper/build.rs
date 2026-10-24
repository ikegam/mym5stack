fn main() {
    // WiFi credentials are baked in at build time (see .env.example)
    println!("cargo:rerun-if-env-changed=WIFI_SSID");
    println!("cargo:rerun-if-env-changed=WIFI_PASS");
    embuild::espidf::sysenv::output();
}
