fn main() {
    // Ensure Cargo recompiles when migration files change, since
    // sqlx::migrate!() embeds them at compile time.
    println!("cargo:rerun-if-changed=migrations");
}
