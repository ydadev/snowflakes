use snowflakes_protocol::Configuration;

#[cfg(windows)]
mod prototype;

fn main() {
    if let Err(error) = run() {
        eprintln!("snowflakes: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--version" => {
            println!("snowflakes {}", env!("CARGO_PKG_VERSION"));
        }
        [flag, path] if flag == "--validate-profile" => {
            let bytes = std::fs::read(path)?;
            let configuration = Configuration::from_json(&bytes)?;
            println!(
                "Valid schema {} profile {} revision {}",
                configuration.schema_version, configuration.profile_id, configuration.revision
            );
        }
        [flag, path] if flag == "--validate-config" => {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take((snowflakes_client::config::FILE_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)?;
            let (_, warnings) = snowflakes_client::config::LocalConfig::parse(&bytes)?;
            println!("Valid local configuration v2; warnings={}", warnings.len());
        }
        #[cfg(windows)]
        [flag] if flag == "--input-probe" => prototype::input_probe()?,
        #[cfg(windows)]
        [flag, seconds] if flag == "--overlay-prototype" => {
            prototype::overlay(seconds.parse()?, false)?;
        }
        #[cfg(windows)]
        [flag, seconds, exercise]
            if flag == "--overlay-prototype" && exercise == "--exercise-recovery" =>
        {
            prototype::overlay(seconds.parse()?, true)?;
        }
        [] => {
            println!("Snowflakes development prototype");
            println!(
                "Usage: snowflakes --version | --validate-profile FILE | --validate-config FILE"
            );
            println!(
                "Windows prototype: --input-probe | --overlay-prototype SECONDS [--exercise-recovery]"
            );
        }
        _ => return Err("unknown command; use --version or --validate-profile FILE".into()),
    }
    Ok(())
}
