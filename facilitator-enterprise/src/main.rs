#[allow(dead_code)]
mod batch;
mod chain;
mod config;
mod enterprise_config;
#[allow(dead_code)]
mod hooks;
mod run;
mod schemes;
mod security;
#[allow(dead_code)]
mod tokens;

use std::process;

use crate::run::run;

#[tokio::main]
async fn main() {
    let result = run().await;
    if let Err(e) = result {
        eprintln!("{e}");
        process::exit(1)
    }
}
