mod cli;
mod core;
mod gui;

use std::process::ExitCode;

use crate::core::i18n::tf;

#[tokio::main]
async fn main() -> ExitCode {
    match cli::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let message = error.to_string();
            eprintln!("{}", tf("error.prefix", &[("message", message.as_str())]));
            ExitCode::FAILURE
        }
    }
}
