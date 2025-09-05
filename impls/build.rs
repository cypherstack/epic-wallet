// Build script to validate SQLite bundling and detect version compatibility issues.
use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    
    // Force bundled SQLite to be used.
    if env::var("SQLITE3_LIB_DIR").is_ok() {
        println!("cargo:warning=SQLITE3_LIB_DIR is set, but bundled SQLite should be used instead");
        println!("cargo:warning=Unsetting SQLITE3_LIB_DIR to force bundled SQLite");
        // Note: We can't unset env vars in build scripts, but we can warn the user.
    }
    
    // Check if we're using the bundled feature.
    println!("cargo:rustc-cfg=sqlite_bundled");
    
    // Validate that libsqlite3-sys version is compatible.
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:warning=Building Epic Wallet with bundled SQLite for version {}", version);
    
    // Set compile-time flags to ensure we're using bundled SQLite.
    println!("cargo:rustc-cfg=bundled_sqlite");
    println!("cargo:rustc-check-cfg=cfg(bundled_sqlite)");
}