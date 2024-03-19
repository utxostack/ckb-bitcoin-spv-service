pub(crate) mod cli;
pub(crate) mod components;
pub(crate) mod prelude;
pub(crate) mod result;
pub(crate) mod utilities;

fn main() -> anyhow::Result<()> {
    cli::Cli::parse().execute()?;
    Ok(())
}
