fn main() {
    // `sqlx::migrate!` embeds the migration list at compile time. Ensure a
    // newly added migration invalidates that expansion instead of leaving a
    // rebuilt native bridge on the previous durable schema.
    println!("cargo:rerun-if-changed=migrations");
}
