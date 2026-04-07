use plugins::CliRegistry;

fn main() {
    if let Err(err) = ur::run(CliRegistry::new()) {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}
