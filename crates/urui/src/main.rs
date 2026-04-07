use plugins::UiRegistry;

fn main() -> anyhow::Result<()> {
    urui::run(UiRegistry::new())
}
