#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use snowflakes_protocol::Configuration;

#[cfg(windows)]
mod prototype;

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("snowflakes: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::*;
        // Preserve redirected pipes. CLI invocations may attach to a parent's
        // console, while a normal GUI launch never creates a console window.
        if !args.is_empty()
            && unsafe { GetStdHandle(STD_OUTPUT_HANDLE).map_or(true, |h| h.is_invalid()) }
        {
            unsafe {
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
        let diagnostic = args.first().is_some_and(|a| {
            matches!(
                a.as_str(),
                "--version"
                    | "--validate-profile"
                    | "--validate-config"
                    | "--input-probe"
                    | "--overlay-prototype"
                    | "--help"
            )
        });
        if !diagnostic {
            return Ok(snowflakes_client::platform::runtime::run(&args));
        }
    }
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
        _ if args.is_empty() || args == ["--help"] => {
            println!("Snowflakes development prototype");
            println!(
                "Usage: snowflakes --version | --validate-profile FILE | --validate-config FILE"
            );
            println!(
                "Windows prototype: --input-probe | --overlay-prototype SECONDS [--exercise-recovery]"
            );
            println!(
                "Windows: --ensure-running [--from-logon], --settings, --server-url URL, --status --json, --pause, --resume, --clear, --set KEY=VALUE, --follow-server on|off, --sync-now, --quit"
            );
            println!("Monitor discovery: --list-monitors --json (does not start the effect)");
        }
        _ => return Err("unknown command; use --version or --validate-profile FILE".into()),
    }
    Ok(0)
}
