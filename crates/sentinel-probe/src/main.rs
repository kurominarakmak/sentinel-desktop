use sentinel_probe::{parse_args, run, ExitCode};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit = match parse_args(&args) {
        Ok(command) => run(command).await,
        Err(error) => {
            eprintln!("sentinel-probe: {error}");
            ExitCode::ValidationFailed
        }
    };
    std::process::exit(exit as i32);
}
