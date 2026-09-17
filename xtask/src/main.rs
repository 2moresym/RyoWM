use std::env;
use std::process::Command;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("check") => {
            println!("cargo build --workspace");
            let status = Command::new("cargo")
                .args(["build", "--workspace"])
                .status()?;
            if !status.success() {
                std::process::exit(1);
            }

            println!("cargo clippy --workspace -- -D warnings");
            let status = Command::new("cargo")
                .args(["clippy", "--workspace", "--", "-D", "warnings"])
                .status()?;
            if !status.success() {
                std::process::exit(1);
            }

            println!("All checks passed.");
            Ok(())
        }
        _ => {
            eprintln!("Usage: cargo xtask check");
            Ok(())
        }
    }
}
